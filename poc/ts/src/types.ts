export type Kind = "prose" | "reasoning" | "tool_call" | "tool_result";

export interface Msg {
  nativeId: string;
  parentId: string | null;
  seq: number;
  role: string;
  kind: Kind;
  ts: number | null;
  isSidechain: boolean;
  text: string;
}

export interface Conv {
  source: string;
  nativeId: string;
  title: string | null;
  cwd: string | null;
  gitBranch: string | null;
  model: string | null;
  forkedFrom: string | null;
  resumeCmd: string | null;
  messages: Msg[];
}

export const parseTs = (v: unknown): number | null => {
  if (typeof v !== "string") return null;
  const t = Date.parse(v);
  return Number.isNaN(t) ? null : t;
};

export const titleFrom = (s: string): string =>
  s.replace(/\s+/g, " ").trim().slice(0, 80);
