import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { walk } from "./walk.ts";
import { type Conv, type Kind, type Msg, parseTs, titleFrom } from "./types.ts";

/** Collapse arbitrary content (string | block | block[]) into searchable text. */
function flatten(v: unknown): string {
  if (v == null) return "";
  if (typeof v === "string") return v;
  if (Array.isArray(v)) return v.map(flatten).filter(Boolean).join("\n");
  if (typeof v === "object") {
    const o = v as Record<string, unknown>;
    for (const k of ["text", "content", "output", "message", "thinking"]) {
      if (k in o) return flatten(o[k]);
    }
    return "";
  }
  return String(v);
}

/** Iterate JSONL lines from a Buffer without materialising a giant split array. */
function* lines(buf: Buffer): Generator<string> {
  let start = 0;
  while (start < buf.length) {
    let nl = buf.indexOf(10, start);
    if (nl === -1) nl = buf.length;
    if (nl > start) yield buf.toString("utf8", start, nl);
    start = nl + 1;
  }
}

// ---------------------------------------------------------------- codex

export function importCodex(path: string): Conv | null {
  const buf = readFileSync(path);
  // `codex resume` writes a new rollout file that reuses the original session_id, so
  // the per-file ordinal must be scoped by the file or ids collide across branches.
  const stem = path.split("/").pop()!.replace(/\.jsonl$/, "");
  let sessionId: string | null = null;
  let cwd: string | null = null;
  let model: string | null = null;
  let forkedFrom: string | null = null;
  let title: string | null = null;
  // A subagent thread shares its parent's session_id and lands in its own rollout file.
  // That is why several files map to one conversation — it is not a resume.
  let isSidechain = false;
  const messages: Msg[] = [];
  let seq = 0;

  for (const line of lines(buf)) {
    let d: any;
    try {
      d = JSON.parse(line);
    } catch {
      continue; // truncated final line on an in-flight session
    }
    const p = d?.payload;
    if (!p || typeof p !== "object") continue;
    const ts = parseTs(d.timestamp);

    if (d.type === "session_meta") {
      sessionId = p.session_id ?? p.id ?? null;
      cwd = p.cwd ?? null;
      model = p.model_provider ?? null;
      forkedFrom = p.forked_from_id ?? null;
      isSidechain = p.thread_source === "subagent" ||
        (typeof p.source === "object" && p.source !== null && "subagent" in p.source);
      continue;
    }

    let role = "";
    let kind: Kind | null = null;
    let text = "";

    if (d.type === "event_msg" && p.type === "user_message") {
      role = "user";
      kind = "prose";
      text = flatten(p.message);
    } else if (d.type === "event_msg" && p.type === "agent_message") {
      role = "assistant";
      kind = "prose";
      text = flatten(p.message);
    } else if (d.type === "response_item" && p.type === "reasoning") {
      role = "assistant";
      kind = "reasoning";
      text = flatten(p.summary ?? p.content);
    } else if (
      d.type === "response_item" &&
      (p.type === "function_call" || p.type === "custom_tool_call")
    ) {
      role = "assistant";
      kind = "tool_call";
      text = `${p.name ?? ""}\n${flatten(p.arguments ?? p.input)}`;
    } else if (
      d.type === "response_item" &&
      (p.type === "function_call_output" || p.type === "custom_tool_call_output")
    ) {
      role = "tool";
      kind = "tool_result";
      text = flatten(p.output);
    } else {
      continue;
    }

    text = text.trim();
    if (!text) continue;
    if (kind === "prose" && role === "user" && !title) title = titleFrom(text);

    messages.push({
      nativeId: `${stem}:${String(seq).padStart(6, "0")}`,
      parentId: null,
      seq,
      role,
      kind,
      ts,
      isSidechain,
      text,
    });
    seq++;
  }

  if (!sessionId || messages.length === 0) return null;
  return {
    source: "codex",
    nativeId: sessionId,
    title,
    cwd,
    gitBranch: null,
    model,
    forkedFrom,
    resumeCmd: `codex resume ${sessionId}`,
    messages,
  };
}

// ---------------------------------------------------------- claude-code

export function importClaudeCode(path: string): Conv | null {
  const buf = readFileSync(path);
  let sessionId: string | null = null;
  let cwd: string | null = null;
  let gitBranch: string | null = null;
  let model: string | null = null;
  let title: string | null = null;
  let customTitle: string | null = null;
  let firstUser: string | null = null;
  const messages: Msg[] = [];
  let seq = 0;

  for (const line of lines(buf)) {
    let d: any;
    try {
      d = JSON.parse(line);
    } catch {
      continue;
    }
    sessionId ??= d.sessionId ?? d.session_id ?? null;
    if (d.cwd) cwd ??= d.cwd;
    if (d.gitBranch) gitBranch ??= d.gitBranch;

    // Title is a fold over an append-only log, not a stored value. A user rename emits
    // `custom-title` (re-emitted on every save, so last wins) and outranks the generated
    // `ai-title`. Both outrank falling back to the first user message.
    if (d.type === "custom-title" && d.customTitle) {
      customTitle = titleFrom(String(d.customTitle));
      continue;
    }
    if (d.type === "ai-title" && d.aiTitle) {
      title = titleFrom(String(d.aiTitle));
      continue;
    }
    if (d.type !== "user" && d.type !== "assistant") continue;
    if (d.isMeta === true) continue; // synthetic system-injected turn

    const m = d.message;
    if (!m) continue;
    if (d.type === "assistant" && m.model) model ??= m.model;

    const ts = parseTs(d.timestamp);
    const sidechain = d.isSidechain === true;
    const parentId = d.parentUuid ?? null;
    const uuid = d.uuid ?? String(seq).padStart(6, "0");

    // content is either a bare string or an array of typed blocks
    const blocks = Array.isArray(m.content)
      ? m.content
      : [{ type: "text", text: m.content }];

    let partIdx = 0;
    for (const b of blocks) {
      let role = d.type as string;
      let kind: Kind;
      let text: string;

      switch (b?.type) {
        case "text":
          kind = "prose";
          text = flatten(b.text);
          break;
        case "thinking":
          kind = "reasoning";
          text = flatten(b.thinking);
          break;
        case "tool_use":
          kind = "tool_call";
          text = `${b.name ?? ""}\n${flatten(b.input) || JSON.stringify(b.input ?? "")}`;
          break;
        case "tool_result":
          kind = "tool_result";
          role = "tool";
          text = flatten(b.content);
          break;
        default:
          continue;
      }

      text = text.trim();
      if (!text) continue;
      if (kind === "prose" && role === "user" && !firstUser) firstUser = titleFrom(text);

      messages.push({
        nativeId: partIdx === 0 ? uuid : `${uuid}#${partIdx}`,
        parentId,
        seq,
        role,
        kind,
        ts,
        isSidechain: sidechain,
        text,
      });
      seq++;
      partIdx++;
    }
  }

  if (!sessionId || messages.length === 0) return null;
  return {
    source: "claude-code",
    nativeId: sessionId,
    title: customTitle ?? title ?? firstUser,
    cwd,
    gitBranch,
    model,
    forkedFrom: null,
    resumeCmd: `claude --resume ${sessionId}`,
    messages,
  };
}

// ------------------------------------------------------------ discovery

export interface SourceFile {
  source: "codex" | "claude-code";
  path: string;
}

export function discover(): SourceFile[] {
  const home = homedir();
  const out: SourceFile[] = [];
  for (const p of walk(join(home, ".codex", "sessions"))) {
    if (p.endsWith(".jsonl") && p.includes("rollout-")) {
      out.push({ source: "codex", path: p });
    }
  }
  for (const p of walk(join(home, ".claude", "projects"))) {
    if (p.endsWith(".jsonl")) out.push({ source: "claude-code", path: p });
  }
  // Directory enumeration order is not guaranteed and differs between runtimes and
  // machines. Several files can feed one conversation, and first-write-wins decides
  // its title — so the order must be pinned or the index is non-deterministic.
  out.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  return out;
}
