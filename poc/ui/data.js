/* Mock data for the chat-search interface prototype.
 *
 * Conversation titles, paths and timings are invented, drawn from this project's own
 * vocabulary. Everything cited as a measurement — 2,963 conversations, 91% tool
 * traffic, the 1.4-6.4 ms query, the 250 ms count settle — is real, from
 * docs/DECISIONS.md and docs/TUI-DESIGN.md.
 *
 * Plain script, no modules, so index.html opens straight from disk.
 */

/* Deterministic PRNG so shapes are stable across reloads. */
function rng(seed) {
  return function () {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/* Categorical encoding, not brand colours — TUI-DESIGN §7. Brand palettes are picked
 * for logos on white and collide badly in a dense list. */
const SOURCES = {
  'claude-code': { hue: 'var(--src-claude-code)', ico: 'var(--ico-claude-code)' },
  codex:         { hue: 'var(--src-codex)',       ico: 'var(--ico-codex)' },
  chatgpt:       { hue: 'var(--src-chatgpt)',     ico: 'var(--ico-chatgpt)' },
  'claude.ai':   { hue: 'var(--src-claude-ai)',   ico: 'var(--ico-claude)' },
  'gemini-cli':  { hue: 'var(--src-gemini)',      ico: 'var(--ico-gemini)' },
};

/* Icon plus colour plus text: three redundant channels, so the row still reads with
 * images off, in greyscale, or for a colourblind reader. TUI-DESIGN §7. */
function sourceBadge(src, cls) {
  const s = SOURCES[src] || {};
  // title, because the row hides the word to buy back width — the name has to stay
  // reachable without opening the conversation.
  return `<span class="badge ${cls || ''}" title="${src}" ` +
         `style="color:${s.hue || 'var(--ink-3)'}">` +
         (s.ico ? `<i class="ico" style="--ico:${s.ico}"></i>` : '') +
         `<span>${src}</span></span>`;
}

const TOOL_CALLS = [
  'Read(schema.rs)', 'Grep(fts_tools)', 'Edit(search.rs)', 'Bash(cargo test)',
  'Read(DECISIONS.md)', 'Write(preview.rs)', 'Glob(crates/**)', 'Bash(cs index)',
  'Read(query.rs)', 'Edit(layout.rs)', 'Bash(cargo build)', 'Read(config.toml)',
];

const USER_LINES = [
  'So the index rebuild wipes it every time — is that actually what we want?',
  'Can you check whether the retention window is what is truncating this?',
  'Walk me through why the date comes out wrong on a DST boundary.',
  'That fixed it. What else reads the same value?',
  'Why is the first keystroke slow and none of the others are?',
  'Does this survive a reindex, or do we recompute the whole thing?',
  'Hold on — does that change the importer contract?',
  'Show me where else this value gets read.',
  'Can we do this without a migration?',
  'What happens on a machine that has never run the archiver?',
];

const ASSISTANT_LINES = [
  'The retention window is 30 days by default, so anything older than the archive floor is already gone.',
  'Because ymd() formats in UTC, a conversation that ended at 21:00 local renders as the next day.',
  'Grouping is by conversation id, so a resumed session folds into one row rather than four.',
  'The scan ceiling is limit × 50, so a broad prefix stops early and reports a floor rather than a total.',
  'Tool traffic is 91% of the corpus text, which is why collapsing it is what makes the pane readable at all.',
  'Every index.db column has to be a pure function of the archive and the importer version — that is what makes rm and rebuild a valid answer.',
  'The first keystroke pays for opening the index; every one after it reuses the same connection.',
  'Nothing derived may enter an id, so surface and project stay out of it entirely.',
  'A failed tool result keeps its text because that is often what makes a conversation recognisable.',
  'Both bounds intersect, so two date tokens give you a range rather than a union.',
];

const REASONING_LINES = [
  'considering whether the window is civil or fixed-width',
  'checking what else reads ended_at',
  'weighing a stored column against a fold',
];

/* Questions the agent asks back. Measured on the real corpus: 4.6% of assistant prose
 * ends in '?', about 1.4 per agent conversation — sparse enough to mark individually. */
const AGENT_QUESTIONS = [
  'Do you want me to keep the old column as a tombstone, or drop it outright?',
  'Should this fail loudly on a missing source, or skip it and report at the end?',
  'That touches the importer contract — do you want me to bump IMPORTER_VERSION?',
];

const MATCH_FRAGMENTS = [
  'panicked: [timezone] offset was None',
  'thread \'main\' [retention] window exceeded',
  'no such column: message.[text_hash]',
  'warning: unused import [fts5]',
  'assertion failed: [ended_at] > started_at',
  'error[E0502]: cannot borrow `*self` as [mutable]',
  'SQLITE_ERROR: no such table: [fts_tools]',
  '[retention]_days = 3650  # was 30',
  'skipped 4 sources: [gemini-cli] had no rows',
  'Deleted 1,204 rows older than the [retention] floor',
  'expected [ended_at] to be non-null, found NULL at row 88',
  'DEBUG scan: 1,903 files, 3.2 GB, [reflink] ok',
  'FAILED tests::[timezone]::dst_boundary_is_23_hours',
  'note: `#[warn(unused)]` on by default for [fts5]_tools',
  '+  pub [text_hash]: [u8; 32],',
  'panic at query.rs:88 — [prefix] index not built',
  'config: [retention]_days must be a positive integer',
  'thread panicked while [grouping] conversations',
  'no rows returned for source [gemini-cli]',
  'assert_eq!(left, right) failed on [ended_date]',
];

/* Kinds mirror cs_core::Kind::ALL exactly.
 *
 * Generated segment-wise rather than by sampling each message independently. Real
 * agentic work is a steer followed by a long unbroken run of tool_call/tool_result
 * pairs; independent sampling interleaves prose through it and destroys exactly the
 * structure the ribbon exists to show. Measured on the corpus: 20.4 messages per
 * steer for claude-code, 37.0 for codex.
 */
function buildMessages(seed, n, matchFractions, agentic, idx) {
  const r = rng(seed);
  const out = [];
  const hits = new Set(matchFractions.map((f) => Math.round(f * (n - 1))));

  let t = 1753000000000;
  let ui = seed % USER_LINES.length;
  let ai = seed % ASSISTANT_LINES.length;
  let qi = seed % AGENT_QUESTIONS.length;
  // Spread by conversation position, not by seed: seeds collide mod pool size
  // and 16 of 24 rows ended up showing the same few fragments.
  let fi = (idx || 0) % MATCH_FRAGMENTS.length;

  const emit = (kind, role, opts) => {
    if (out.length >= n) return;
    const o = opts || {};
    const i = out.length;
    const len =
      kind === 'prose' ? 80 + r() * 420
      : kind === 'reasoning' ? 300 + r() * 1500
      : 1500 + r() * 48000;

    t += 20000 + r() * 90000;
    const paused = i > 0 && r() > 0.985;
    const gapShown = paused ? (4 + r() * 20) * 3600000 : 0;
    if (paused) t += gapShown;

    const tool = kind === 'tool_call' || kind === 'tool_result'
      ? o.tool || TOOL_CALLS[Math.floor(r() * TOOL_CALLS.length)] : null;
    const isQuestion = Boolean(o.question);
    const match = hits.has(i);

    out.push({
      i, kind, len, role, ts: t, paused, gapShown, isQuestion, tool,
      onPath: r() > 0.05,
      err: kind === 'tool_result' && r() > 0.94,
      isSidechain: Boolean(o.sidechain),
      match,
      frag: match ? MATCH_FRAGMENTS[(fi = (fi + 1) % MATCH_FRAGMENTS.length)] : null,
      // Full zoom shows the paragraph; outline shows only its first line. Without a
      // long form the two middle stops render identically.
      textLong: kind === 'prose' && role !== 'user'
        ? [0, 1, 2].map((k) => ASSISTANT_LINES[(ai + k) % ASSISTANT_LINES.length]).join(' ')
        : null,
      text:
        kind === 'prose'
          ? role === 'user'
            ? USER_LINES[(ui = (ui + 1) % USER_LINES.length)]
            : isQuestion
            ? AGENT_QUESTIONS[(qi = (qi + 1) % AGENT_QUESTIONS.length)]
            : ASSISTANT_LINES[(ai = (ai + 1) % ASSISTANT_LINES.length)]
          : kind === 'reasoning'
          ? REASONING_LINES[Math.floor(r() * REASONING_LINES.length)]
          : tool,
    });
  };

  while (out.length < n) {
    emit('prose', 'user');                              // the steer
    if (r() > 0.6) emit('reasoning', 'assistant');

    if (agentic) {
      // One unbroken run of tool pairs — this is what makes a ribbon block.
      const pairs = 3 + Math.floor(r() * 14);
      const side = r() > 0.82;                          // a subagent excursion
      for (let k = 0; k < pairs && out.length < n; k++) {
        const tool = TOOL_CALLS[Math.floor(r() * TOOL_CALLS.length)];
        emit('tool_call', 'assistant', { tool, sidechain: side });
        emit('tool_result', 'tool', { tool, sidechain: side });
      }
      if (r() > 0.75) emit('reasoning', 'assistant');
    } else {
      if (r() > 0.75) { const tl = TOOL_CALLS[Math.floor(r() * TOOL_CALLS.length)];
        emit('tool_call', 'assistant', { tool: tl }); emit('tool_result', 'tool', { tool: tl }); }
    }

    // 4.6% of assistant prose ends in a question, measured.
    emit('prose', 'assistant', { question: r() > 0.88 });
    if (!agentic && r() > 0.5) emit('prose', 'assistant');
  }

  return out.slice(0, n);
}

/* A segment is a steer plus the run it caused. This is the fold unit and the ribbon's
 * unit — the corpus says 2-8 of them typically, 9-20 sometimes. */
function segmentize(msgs) {
  const segs = [];
  let cur = null;
  const push = () => { if (cur) segs.push(cur); };

  msgs.forEach((m) => {
    const isSteer = m.kind === 'prose' && m.role === 'user';
    if (isSteer || !cur) {
      const prev = cur;
      push();
      cur = {
        steer: isSteer ? m : null,
        items: [],
        calls: 0, fails: 0, questions: 0, matches: 0,
        // A break is a >4h pause; the ribbon draws it as a notch.
        gapMs: prev && prev.items.length && m.ts
          ? m.ts - (prev.items[prev.items.length - 1].ts || m.ts)
          : 0,
        start: m.i,
      };
      if (isSteer) {
        if (m.match) cur.matches++;
        cur.end = m.i;
        return;
      }
    }
    cur.items.push(m);
    cur.end = m.i;
    if (m.kind === 'tool_call') cur.calls++;
    if (m.err) cur.fails++;
    if (m.isQuestion) cur.questions++;
    if (m.match) cur.matches++;
  });
  push();
  return segs;
}

const RAW = [
  ['claude-code', '~/dev/projects/chat-search', 142, '3d', 0, 11,
   'Retention window: raise the Claude Code transcript TTL to 3650 days',
   '2026-07-26 → 2026-07-31', [0.06, 0.09, 0.42, 0.61, 0.63, 0.88]],
  ['codex', '~/dev/projects/chat-search', 38, '5d', 2, 23,
   'Why does ymd() render UTC instead of the local day?', '2026-07-29', [0.12, 0.34, 0.37, 0.71]],
  ['claude-code', '~/dev/projects/chat-search', 96, '4d', 0, 103,
   'Prefix index: why the first keystroke costs 60 ms and the rest do not',
   '2026-07-30', [0.11, 0.29, 0.68, 0.72]],
  ['claude.ai', '—', 24, '1w', 0, 31,
   'FTS5 external-content tables and the contentless rowid trap', '2026-07-24', [0.22, 0.55]],
  ['claude-code', '~/dev/projects/chat-search', 211, '1w', 3, 47,
   'Grouping conversations that span several transcript files',
   '2026-07-19 → 2026-07-25', [0.04, 0.17, 0.19, 0.48, 0.52, 0.55, 0.83, 0.91]],
  ['codex', '~/dev/projects/chat-search', 41, '6d', 0, 109,
   'Archive scanner misses files written while the scan is running', '2026-07-28', [0.19, 0.61]],
  ['chatgpt', '—', 16, '2w', 0, 59,
   'sqlite-vec in the index, or embeddings in their own file?', '2026-07-16', [0.31, 0.62, 0.66]],
  ['claude-code', '~/dev/projects/chat-search', 128, '2w', 1, 113,
   'match_seqs and the ten-cell density strip', '2026-07-17 → 2026-07-18',
   [0.07, 0.23, 0.41, 0.79, 0.94]],
  ['claude-code', '~/dev/sandbox/eval-harness', 74, '2w', 0, 127,
   'Eval harness: judging the 24 seed queries by hand', '2026-07-15', [0.33, 0.57, 0.86]],
  ['chatgpt', '—', 29, '2w', 0, 149,
   'How do other people store a civil date without a timezone?', '2026-07-14', [0.18, 0.52]],
  ['claude-code', '~/dev/projects/chat-search', 87, '3w', 0, 71,
   "Borrow checker fight in the archive scanner's manifest writer",
   '2026-07-08 → 2026-07-09', [0.09, 0.44, 0.77]],
  ['codex', '~/dev/projects/chat-search', 33, '3w', 0, 131,
   'Destination as an enum: Terminal{argv} against Web{url}', '2026-07-11', [0.24, 0.66]],
  ['claude.ai', '—', 47, '3w', 0, 151,
   'Is a daemon worth 3 ms? Arguing both sides', '2026-07-10', [0.28, 0.49, 0.71]],
  ['claude-code', '~/dev/projects/chat-search', 58, '5w', 0, 137,
   'Title fold: stripping harness markup out of 42 conversations', '2026-06-25', [0.15, 0.48, 0.81]],
  ['codex', '~/dev/…/chat-search', 53, '1mo', 0, 83,
   'Codex resume ids collide across sessions', '2026-06-28', [0.14, 0.51, 0.88]],
  ['claude-code', '~/dev/projects/chat-search', 164, '6w', 2, 157,
   'Importer purity: no filesystem access inside a parser', '2026-06-18 → 2026-06-20',
   [0.05, 0.31, 0.44, 0.67, 0.9]],
  ['chatgpt', '—', 21, '6w', 0, 163,
   'BM25 with time decay — what does k1 actually control?', '2026-06-17', [0.36, 0.74]],
  ['gemini-cli', '~/dev/projects/chat-search', 19, '2mo', 0, 97,
   'Porter stemming, and why the preview highlight misses', '2026-06-02', [0.27, 0.73]],
  ['claude-code', '~/dev/projects/chat-search', 112, '2mo', 0, 167,
   'Source ids are permanent; nothing derived may enter one', '2026-05-29', [0.13, 0.39, 0.62, 0.85]],
  ['claude.ai', '—', 35, '2mo', 0, 173,
   'Append-only event log, folded last-write-wins', '2026-05-24', [0.21, 0.58]],
  ['codex', '~/dev/projects/chat-search', 64, '3mo', 0, 179,
   'Reflink copies on APFS, and when they silently fall back', '2026-05-02', [0.17, 0.53, 0.82]],
  ['claude-code', '~/dev/projects/chat-search', 203, '3mo', 4, 181,
   'The DAG: parent_id, on_head_path, and edited-away branches',
   '2026-04-27 → 2026-05-01', [0.03, 0.22, 0.36, 0.58, 0.74, 0.93]],
  ['chatgpt', '—', 18, '4mo', 0, 191,
   'Explain FTS5 contentless tables like I have not read the docs', '2026-04-08', [0.4, 0.79]],
  ['claude-code', '~/dev/projects/chat-search', 91, '5mo', 0, 193,
   'Archive first, parse never: what the scanner must not know', '2026-03-11', [0.16, 0.47, 0.76]],
];


/* ============================================================ real data ====
 * poc/ui/export.py writes real-data.js (gitignored — it is conversation text).
 * When it is present the fixtures below are ignored entirely and the prototype runs
 * on the actual corpus. This is the whole point: the generator here was wrong once
 * already, in exactly the dimension the ribbon exists to show.
 */

const KIND_FROM = { p: 'prose', r: 'reasoning', c: 'tool_call', o: 'tool_result' };
const FOUR_H = 4 * 3600 * 1000;

function adaptReal(data) {
  return data.conversations.map((c, idx) => {
    let prev = null;
    const msgs = c.msgs.map((m, i) => {
      const kind = KIND_FROM[m.k] || 'prose';
      const role = m.r === 'u' ? 'user' : 'asst';
      const text = (m.x || '').replace(/\s+/g, ' ').trim();
      const gap = prev && m.t && prev.ts ? m.t - prev.ts : 0;
      const out = {
        i, kind, role,
        // cs's two answers about this message, carried by export.py and never recomputed
        // here: which band it is in, and whether a reader draws it at all. `drawn` is
        // false for a successful tool result — the call implies it — and that is what the
        // ribbon's axis is made of, so a second opinion about it here would put the bands
        // and the marks over the same axis in two different coordinate spaces.
        band: m.b || null,
        drawn: !m.nd,
        len: Math.max(m.n || text.length || 1, 1),
        ts: m.t || null,
        paused: gap > FOUR_H,
        gapShown: gap > FOUR_H ? gap : 0,
        // No question-detection in the schema; '?' is the same cheap heuristic the
        // 4.6% measurement used, and it is honest about being one.
        // Two signals, one mark. `request_user_input` is the agent formally blocking on
        // you — exact, and rare: 237 calls in 92 conversations. The '?' heuristic is
        // broader and noisier (1,402 in 578) but catches the ordinary asked-a-question
        // case that no tool records. The union is honest about both.
        isQuestion: (kind === 'prose' && role === 'asst' && /\?\s*$/.test(text)) ||
                    (kind === 'tool_call' &&
                     /request_user_input|AskUserQuestion/.test(m.c || '')),
        tool: kind === 'tool_call' || kind === 'tool_result' ? (text || 'tool') : null,
        // Carried from the export rather than parsed out of the truncated preview text.
        call: m.c || null,
        // What the call actually was. A `Bash` line used to say "Bash" and nothing else,
        // which is the one thing you do not need to be told.
        arg: m.a || null,
        diff: m.d || null,
        act: kind === 'tool_call' ? actOf(m.c) : null,
        onPath: true,
        err: Boolean(m.e),
        isSidechain: Boolean(m.s),
        // The context was cut here: everything above is a summary from this point on.
        compacted: Boolean(m.z),
        match: false, frag: null,
        text: text || (kind === 'tool_result' ? '' : '…'),
        textLong: kind === 'prose' && role !== 'user' ? text : null,
      };
      if (m.t) prev = out;
      return out;
    });

    const users = msgs.filter((m) => m.kind === 'prose' && m.role === 'user');
    const ageDaysFrom = (ts) => (Date.now() - ts) / 86400000;
    const d = c.ended_at ? ageDaysFrom(c.ended_at) : 0;
    const age = d < 1 ? 'today' : d < 7 ? Math.round(d) + 'd'
      : d < 31 ? Math.round(d / 7) + 'w' : Math.round(d / 30) + 'mo';

    return {
      id: c.id,
      src: c.source === 'chatgpt-export' ? 'chatgpt' : c.source,
      dir: c.cwd || '—',
      // Normalised in the export, not here: a worktree is the project it was cut from,
      // and a Codex scratch directory is not a project at all. See export.py::project_of.
      project: c.project || null,
      n: msgs.length,
      age,
      endedAt: c.ended_at,
      forks: c.thread_count || 0,
      seed: idx,
      title: c.title || (users[0] ? users[0].text.slice(0, 90) : '(untitled)'),
      titleOrigin: c.title_origin,
      span: c.started_at && c.ended_at
        ? new Date(c.started_at).toISOString().slice(0, 10) + ' → ' +
          new Date(c.ended_at).toISOString().slice(0, 10)
        : '',
      matches: [],
      msgs,
      // The shape, exactly as `cs search --json` emitted it: [[band, count], …] over the
      // drawn messages in reading order. The ribbon draws these runs rather than encoding
      // its own; the fixtures below have no index behind them and so have no `shape`,
      // which is what app.js's fallback is for.
      shape: c.runs || null,
      segments: segmentize(msgs),
      model: c.model || null,
      files: c.files || [],
      // Only 57 conversations in the corpus contain any subagent traffic — but where it
      // appears it averages 52% of the conversation and reaches 100%. That is a fact
      // about the whole conversation, not a tint on scattered messages.
      sidechain: msgs.length ? msgs.filter((m) => m.isSidechain).length / msgs.length : 0,
      // Summed over the conversation: how much of the file tree this actually moved.
      // 160 of 354 sampled conversations changed a line; the rest only ran and read.
      edits: msgs.reduce((a, m) => (m.diff ? [a[0] + m.diff[0], a[1] + m.diff[1]] : a), [0, 0]),
      topics: TOPICS_BY_CONV.get(c.id) || [],
      bestMatch: null,
    };
  });
}

/* Substring search over the exported text. Not BM25 and not pretending to be — it is
 * enough to place real match ticks at real positions, which is what the ribbon needs
 * to be judged. */
function applyQuery(convs, q) {
  const terms = (q || '').toLowerCase().split(/\s+/).filter((t) => t.length > 1);
  convs.forEach((c) => {
    let best = null;
    c.msgs.forEach((m) => {
      m.match = false;
      m.frag = null;
      if (!terms.length || !m.text) return;
      const low = m.text.toLowerCase();
      const at = terms.map((t) => low.indexOf(t)).filter((i) => i >= 0);
      if (!at.length) return;
      m.match = true;
      const start = Math.max(0, Math.min(...at) - 34);
      const hit = terms.find((t) => low.includes(t));
      m.frag = m.text.slice(start, start + 110)
        .replace(new RegExp('(' + hit.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + ')', 'i'), '[$1]');
      if (!best) best = m;
    });
    c.bestMatch = best;
    c.matchCount = c.msgs.filter((m) => m.match).length;
  });
  return convs;
}

/* What a tool call was *for*, not which tool it was. Measured over the corpus: 47,479
 * calls run something, 9,460 change a file, 4,502 look at one, 1,164 steer. "Was this
 * exploring, editing or executing?" is a different question from "was this tool
 * traffic", and the single tool band cannot answer it.
 *
 * Names, not capabilities: the same act wears a different name per harness — codex says
 * `exec_command`, claude-code says `Bash`, and both mean run. */
const ACTS = {
  look:   ['Read', 'Glob', 'Grep', 'ToolSearch', 'WebFetch', 'WebSearch', 'view_image',
           'ReadMcpResourceTool', 'ListMcpResourcesTool', 'NotebookRead'],
  change: ['Edit', 'Write', 'apply_patch', 'NotebookEdit', 'MultiEdit'],
  run:    ['exec_command', 'Bash', 'shell', 'exec', 'shell_command', 'write_stdin',
           'wait', 'js', 'python', 'container.exec'],
  steer:  ['update_plan', 'TodoWrite', 'request_user_input', 'AskUserQuestion',
           'ExitPlanMode', 'Task'],
};
const ACT_OF = {};
Object.entries(ACTS).forEach(([act, names]) => names.forEach((n) => { ACT_OF[n] = act; }));

const ACT_GLYPH = { look: '◎', change: '✎', run: '›', steer: '⚑', other: '⚙' };

/* Unrecognised tools fall to `run`, not to a fifth bucket: an unknown tool in an agent
 * transcript is overwhelmingly something being executed, and a bucket called "other"
 * on the ribbon would be a colour that means nothing. */
const actOf = (name) => ACT_OF[(name || '').split(/[\s(]/)[0]] || 'run';

const REAL = typeof window !== 'undefined' && window.REAL_DATA ? window.REAL_DATA : null;

/* Rolled up over the whole index rather than over the sample, so a project row states
 * how big the project actually is and the export says how much of it it carried. The
 * topic rail does not do this yet, and reads thinner than the corpus is because of it. */
const PROJECTS = (REAL && REAL.projects) || [];
const PROJECT_BY_PATH = new Map(PROJECTS.map((p) => [p.path, p]));

const AGENTIC_SOURCES = new Set(['claude-code', 'codex', 'gemini-cli']);

/* Topic membership, inverted once. The topics arrive as member lists, and asking
 * "which topics is this conversation in" per row over 354 rows against 26 lists is a
 * scan nobody needs to repeat. */
const TOPICS_BY_CONV = new Map();
((REAL && REAL.topics) || []).forEach((t) => {
  t.members.forEach((id) => {
    if (!TOPICS_BY_CONV.has(id)) TOPICS_BY_CONV.set(id, []);
    TOPICS_BY_CONV.get(id).push(t.name);
  });
});

const CONVERSATIONS = REAL
  // Export order is stratified by source, which put every ChatGPT conversation first.
  // A blank query is a recency list, not a grouped-by-tool list.
  ? adaptReal(REAL).sort((a, b) => (b.endedAt || 0) - (a.endedAt || 0))
  : RAW.map((r, i) => {
  const msgs = buildMessages(r[5], r[2], r[8], AGENTIC_SOURCES.has(r[0]), i);
  return {
    id: 'c' + (i + 1),
    src: r[0], dir: r[1], n: r[2], age: r[3], forks: r[4], seed: r[5],
    title: r[6], span: r[7], matches: r[8],
    msgs,
    segments: segmentize(msgs),
    // The row's line 3: the true best match, so it can never disagree with the ribbon.
    bestMatch: msgs.find((m) => m.match) || null,
  };
});

/* Sources come from config ∪ index, so a source with zero rows is visible rather than
 * absent — TUI-DESIGN §5. A broken importer must not read as a tool you never used. */
const SOURCE_ROWS = [
  { k: 'claude-code', n: 1204, state: 'ok' },
  { k: 'codex', n: 421, state: 'ok' },
  { k: 'chatgpt', n: 2044, state: 'ok' },
  { k: 'claude.ai', n: 188, state: 'ok' },
  { k: 'gemini-cli', n: 0, state: 'zero' },
  { k: 'opencode', n: null, state: 'unconfigured' },
];

const COLLECTIONS = [
  { t: 'Native app design', n: 31 },
  { t: 'Retention & capture', n: 58 },
  { t: 'Ranking quality', n: 24 },
  { t: 'Rust, learning', n: 96 },
];

const CLUSTERS = [
  { t: 'SQLite / FTS5 internals', n: 74 },
  { t: 'Importer edge cases', n: 41 },
  { t: 'Terminal UI work', n: 39 },
  { t: 'Dates and timezones', n: 22 },
];

/* token is what gets written into the query string when the row is clicked —
 * the same grammar cs_core::query already parses. */
const WHEN_ROWS = [
  { t: 'Today', n: 12, token: 'today' },
  { t: 'This week', n: 96, token: 'week' },
  { t: 'This month', n: 402, token: 'month' },
  { t: 'Older', n: 2453, token: '>1mo' },
];

/* A sitting is an ended_at window of about two hours. No model, no embeddings —
 * a self-join on a column that already exists, reaching the 69% of the corpus that
 * is ChatGPT and carries no cwd. */
const SITTING_DAYS = [
  {
    day: 'Wednesday 29 July', span: '14:02 → 18:41',
    items: [
      ['14:02', 'chatgpt', 11, 201, 'Timezone arithmetic — is a civil day always 86,400 seconds?'],
      ['14:07', 'claude-code', 64, 202, 'Render the local day once in cs-core, not in three clients'],
      ['15:31', 'codex', 38, 203, 'ymd() renders UTC instead of the local day'],
      ['15:34', 'claude-code', 22, 204, 'Pin DST arithmetic at 82,800 and 90,000 seconds'],
      ['16:52', 'claude.ai', 9, 205, 'How do other tools store a civil date without a timezone?'],
      ['18:41', 'claude-code', 17, 206, 'Close the date bug, update DECISIONS.md and the ADR table'],
    ],
  },
  {
    day: 'Monday 27 July', span: '09:14 → 12:40',
    items: [
      ['09:14', 'claude-code', 48, 211, 'Why does a bare cs index miss 69% of the corpus?'],
      ['09:18', 'chatgpt', 14, 212, 'What does a ChatGPT export actually contain?'],
      ['10:36', 'claude-code', 92, 213, 'Fold the export path into the default scan'],
      ['12:40', 'codex', 26, 214, 'IndexStats counted objects, not rows'],
    ],
  },
  {
    day: 'Thursday 23 July', span: '20:05 → 22:58',
    items: [
      ['20:05', 'claude.ai', 19, 221, 'Talk me out of building a daemon'],
      ['20:09', 'claude-code', 71, 222, 'Measure spawn plus index open, honestly'],
      ['21:44', 'claude-code', 40, 223, 'Write up ADR 14 with its revisit trigger'],
      ['22:58', 'codex', 12, 224, 'Retire the fzf script the TUI replaced'],
    ],
  },
];

if (REAL && REAL.sittings && REAL.sittings.length) {
  // Real sittings, bounded in both gap and total span — the gap rule alone chained
  // eighteen conversations across a whole day into one "sitting".
  SITTING_DAYS.length = 0;
  REAL.sittings.forEach((s) => {
    const day = new Date(s.start);
    SITTING_DAYS.push({
      day: day.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' }),
      span: new Date(s.start).toTimeString().slice(0, 5) + ' → ' +
            new Date(s.end).toTimeString().slice(0, 5),
      items: s.items.map((it, ii) => {
        const conv = CONVERSATIONS.find((c) => c.id === it.id);
        return {
          // Carried so a sitting card can open the same preview drawer the other two
          // views use. Without it a sitting was the one place you could see that a
          // conversation existed and not read it.
          id: conv ? conv.id : null,
          t: new Date(it.ended_at).toTimeString().slice(0, 5),
          src: it.source === 'chatgpt-export' ? 'chatgpt' : it.source,
          n: it.msg_count,
          title: it.title || '(untitled)',
          msgs: conv ? conv.msgs : [],
        };
      }),
    });
  });
}

// Only the fixtures need expanding; real sittings arrive already shaped, and
// destructuring those objects as arrays is what threw "object is not iterable".
if (!REAL || !REAL.sittings || !REAL.sittings.length) {
  SITTING_DAYS.forEach((d, di) => {
    d.items = d.items.map(([t, src, n, seed, title], ii) => ({
      t, src, n, seed,
      title,
      msgs: buildMessages(seed, n, [0.2, 0.55, 0.8], AGENTIC_SOURCES.has(src), di * 7 + ii),
    }));
  });
}

/* Annotation overlay. Each pin anchors to a live element by selector, so the notes sit
 * on the interface rather than in a document beside it. */
const ANNOTATIONS = [
  { sel: '#qbox', side: 'below', t: 'Query text is the only filter state',
    d: 'Clicking a source rewrites this string. TUI-DESIGN §5 caught fast-resume holding a second source of truth and needing six reconciliation methods to keep them agreeing.' },
  { sel: '#stat', side: 'below', t: 'The count settles, it does not stream',
    d: '50 of 655 arrives 250 ms after typing stops. Charging it per keystroke spends 36 ms at the one moment nobody is reading the number.' },
  { sel: '[data-anno="sources"]', side: 'right', t: 'Zero-row sources stay visible',
    d: 'gemini-cli is dim with a warning and a count of zero; opencode is on the machine but unconfigured. A broken importer must never look like a tool you simply never used.' },
  { sel: '[data-anno="micro"]', side: 'right', t: 'The row map resolves to the conversation',
    d: 'match_density renders ten fixed buckets because a terminal has ten cells. Here a 19-message conversation gets 19 columns and a 211-message one gets 74 — resolution set by the data, not the grid.' },
  { sel: '[data-anno="dir"]', side: 'right', t: 'Paths middle-elide, titles tail-elide',
    d: 'The leaf is the discriminating token in a path, so it survives. The front of a title is what discriminates, so titles truncate the other way.' },
  { sel: '#mm', side: 'left', t: 'The minimap is the scrollbar',
    d: 'Drag it. Band tone is kind, height is log(bytes), indent is role, and match ticks ride the outer edge in the loudest colour on screen.' },
  { sel: '[data-anno="tool"]', side: 'left', t: 'Tool results are omitted, failures are not',
    d: 'Read(schema.rs) already implies a result, so a line reporting 1.2 KB spends a row repeating it. An error keeps its text — a tool breaking is recognition information.' },
  { sel: '[data-anno="views"]', side: 'below', t: 'Sittings is a view, not a search',
    d: 'A ranked list sorts by relevance. A sitting is adjacency in time, which is the only cheap cross-tool signal and needs no model at all.' },
  { sel: '[data-anno="rule"]', side: 'right', t: 'A collection is a rule, not a list',
    d: 'matches(query) ∪ pinned − excluded. Storing the rule rather than the membership is what lets a collection survive a reindex, and the arithmetic is shown because you will want to know why something is in here.' },
  { sel: '[data-anno="proposal"]', side: 'left', t: 'A cluster is a proposal, a collection is a decision',
    d: 'Clustering built as an answer gets switched off the first time it is wrong. Built as a proposal machine, being wrong costs one dismissal. Accepting freezes the current members into a rule you own.' },
];
