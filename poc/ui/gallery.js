/* chat-search — component gallery.
 *
 * Every component, in every state, in both themes, on one page. It exists because the
 * prototype makes you earn each state: to see a fidelity chip switched off you run the
 * app, pick a conversation and click twice; to compare the four kind colours you first
 * have to find a conversation that contains all four; to see an empty state you have to
 * construct a query that returns nothing. Polish decisions were being made five clicks
 * apart and from memory.
 *
 * Two rules keep it honest:
 *
 *   1. It renders the app's own functions from window.CS_UI, never a copy of its markup.
 *      A second copy would drift, and a drifted gallery is worse than none — you would
 *      be judging something that is not what ships.
 *   2. Its own CSS may only lay out and label. Every component here is styled by
 *      styles.css, so a token change shows up here immediately and truthfully.
 *
 * Fixtures are synthetic and built in this file, so the page is committable: none of
 * the archive's text is in it. Plain script, no modules, no build — same as the rest.
 */
(function () {
  'use strict';

  const UI = window.CS_UI;
  const $ = (id) => document.getElementById(id);
  const el = (tag, cls) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    return n;
  };

  /* ------------------------------------------------------------ fixtures -- */

  const T0 = Date.parse('2026-08-02T14:20:00');

  /* A message spec is a terse string so a whole conversation shape fits on one line and
     can be read as a shape: `u` steer, `a` agent prose, `r` reasoning, `c` tool call,
     `o` tool result. Suffixes mark the overlays — `!` failed, `?` asked you, `~` paused,
     `#` compaction, `*` matched — and `:look` / `:change` set the act. */
  function msg(spec, i) {
    const [body, act] = spec.split(':');
    const k = body[0];
    const flags = body.slice(1);
    const kind = { u: 'prose', a: 'prose', r: 'reasoning', c: 'tool_call', o: 'tool_result' }[k];
    const role = k === 'u' ? 'user' : k === 'o' ? 'tool' : 'asst';
    const call = k === 'c' ? ({ look: 'Read', change: 'Edit', run: 'Bash' }[act] || 'Bash') : null;
    return {
      i, kind, role,
      len: kind === 'prose' ? (role === 'user' ? 210 : 940) : kind === 'reasoning' ? 190 : 520,
      ts: T0 + i * 42000,
      paused: flags.includes('~'),
      gapShown: flags.includes('~') ? 6.5 * 3600000 : 0,
      isQuestion: flags.includes('?'),
      compacted: flags.includes('#'),
      tool: call ? `${call}(…)` : null,
      call,
      arg: k === 'c'
        ? ({ look: 'crates/cs-core/src/preview.rs',
             change: 'crates/cs-core/src/search.rs',
             run: 'cargo test --workspace 2>&1 | tail -30' }[act] || 'cargo test --workspace')
        : null,
      diff: act === 'change' ? [4, 11] : null,
      act: k === 'c' ? (act || 'run') : null,
      onPath: !flags.includes('^'),
      err: flags.includes('!'),
      isSidechain: flags.includes('&'),
      match: flags.includes('*'),
      frag: flags.includes('*') ? 'no such column: message.[text_hash]' : null,
      text: k === 'u' ? 'Does this survive a reindex, or do we recompute the whole thing?'
        : k === 'a' ? 'Every index.db column has to be a pure function of the archive and the importer version — that is what makes rm and rebuild a valid answer.'
        : k === 'r' ? 'weighing a stored column against a fold'
        : k === 'c' ? `${call}(…)`
        : flags.includes('!') ? 'Error: no such column: message.text_hash' : '',
      textLong: k === 'a'
        ? 'Every index.db column has to be a pure function of the archive and the importer version — that is what makes rm and rebuild a valid answer. Anything authored or generated belongs in library.db instead, which is append-only and never rebuilt. The rule is what lets `rm index.db && cs index` stay a correct answer to any schema change.'
        : null,
    };
  }

  let seq = 0;
  function conv(spec, over) {
    const msgs = spec.trim().split(/\s+/).map(msg);
    const o = over || {};
    const c = Object.assign({
      id: 'g' + (++seq),
      src: 'claude-code',
      dir: '/Users/tylercrosse/dev/projects/chat-search',
      project: '/Users/tylercrosse/dev/projects/chat-search',
      n: msgs.length,
      age: '2d',
      endedAt: T0,
      forks: 1,
      seed: seq,
      title: 'Render the local day once in cs-core, not in three clients',
      titleOrigin: 'generated',
      span: '2026-08-01 → 2026-08-02',
      model: 'claude-opus-5',
      files: ['crates/cs-core/src/search.rs', 'docs/DECISIONS.md'],
      topics: ['Rust and cargo', 'SQLite and FTS5'],
      sidechain: 0,
      matches: [],
      msgs,
      bestMatch: msgs.find((m) => m.match) || null,
    }, o);
    c.n = o.n || msgs.length;
    c.segments = typeof segmentize === 'function' ? segmentize(msgs) : [];
    c.edits = msgs.reduce((a, m) => (m.diff ? [a[0] + m.diff[0], a[1] + m.diff[1]] : a), [0, 0]);
    return c;
  }

  /* Borrow a piece of app state for the length of one specimen and put it back, the
     same bargain `rib()` makes with state.fidelity. Rendering the app's own functions
     means rendering them against the app's own state, so the states a component has
     have to be posed rather than described. */
  function withQuery(on, build) {
    const keep = UI.state.query.free;
    UI.state.query.free = on ? 'text_hash' : '';
    try {
      return build();
    } finally {
      UI.state.query.free = keep;
    }
  }

  /* Repeat a shape to a length. The ribbon's log scaling only shows itself across a
     real spread, and the corpus spans 4 to 2,553 messages. */
  const rep = (unit, times) => new Array(times).fill(unit.trim()).join(' ');

  /* ------------------------------------------------------------ scaffold -- */

  const SECTIONS = [];
  function section(id, title, meta, note, build) {
    SECTIONS.push({ id, title, meta, note, build });
  }

  function specimen(caption, node, plain) {
    const w = el('div', 'gal-item');
    if (caption) {
      const c = el('div', 'cap');
      c.textContent = caption;
      w.appendChild(c);
    }
    const f = el('div', 'gal-frame' + (plain ? ' plain' : ''));
    f.appendChild(node);
    w.appendChild(f);
    return w;
  }

  const caseOf = (label, node) => {
    const w = el('div', 'gal-case');
    if (label) {
      const l = el('div', 'gal-lab');
      l.textContent = label;
      w.appendChild(l);
    }
    w.appendChild(node);
    return w;
  };

  const stack = (nodes) => {
    const d = el('div');
    nodes.forEach((n) => d.appendChild(n));
    return d;
  };

  /* ------------------------------------------------------------ sections -- */

  section('ribbon', 'Ribbon', 'one graphic, two channels',
    'The row\'s density strip. Body is shape and query-independent, run-length encoded ' +
    'over consecutive same-kind messages; the overlay is match ticks and vanishes with ' +
    'no query. The four kinds sit on a <b>luminance ramp</b> — 2.2 / 4.0 / 7.2 / 13.0 ' +
    'against the track, an even ~1.8× per step — because the previous palette separated ' +
    'them by hue only, and hue is the channel that degrades fastest at 2px.',
    () => {
      const out = el('div');

      out.appendChild(caseOf('kinds — each alone, then interleaved', stack([
        specimen('you (13.0 : 1 … the steer, floored at 2px so it stays a separator)',
          rib(conv(rep('u', 14)))),
        specimen('agent (7.2 : 1)', rib(conv(rep('a', 14)))),
        specimen('reasoning (4.0 : 1)', rib(conv(rep('r', 14)))),
        specimen('tools (2.2 : 1 — quietest, because they are 66–85% of the volume)',
          rib(conv(rep('c o', 14)))),
        specimen('a real agentic shape: steer, then the run it caused',
          rib(conv('u ' + rep('c o', 9) + ' a u ' + rep('c o', 7) + ' a'))),
      ])));

      out.appendChild(caseOf('dimmed — a hidden kind stays on the axis', stack([
        specimen('tools + reasoning hidden (the `segments` default) at 0.55 opacity',
          rib(conv('u ' + rep('c o', 9) + ' a u ' + rep('r c o', 5) + ' a'),
              { fidelity: { user: 'expanded', agent: 'collapsed', reason: 'hidden', tool: 'hidden' } })),
        specimen('the same conversation, nothing hidden',
          rib(conv('u ' + rep('c o', 9) + ' a u ' + rep('r c o', 5) + ' a'))),
      ])));

      out.appendChild(caseOf('acts — runs break on what the call was for', stack([
        specimen('look 1.7 · run 2.2 · change 3.1 — sub-shades inside the tool band, ' +
                 '1.29× and 1.42× apart',
          rib(conv('u ' + rep('c:look o', 4) + ' ' + rep('c:run o', 6) + ' ' +
                   rep('c:change o', 3) + ' ' + rep('c:run o', 5) + ' a'))),
        specimen('a patch landing inside a wall of exec — the reason acts split the run',
          rib(conv('u ' + rep('c:run o', 12) + ' c:change o ' + rep('c:run o', 10) + ' a'))),
      ])));

      out.appendChild(caseOf('overlays', stack([
        specimen('match ticks (amber, query-dependent)',
          rib(conv('u c o c* o a c o c* o c o a'), { q: true })),
        specimen('pause over 4h (thin, partial height) · compaction (doubled, full height)',
          rib(conv('u c o a u~ c o a c o u# a c o a'))),
        specimen('failed call (red) · agent asked you (teal dot)',
          rib(conv('u c o c o! a? c o c o! a c o a'))),
      ])));

      out.appendChild(caseOf('length — the track is the scale cue', stack([
        [10, 91, 344, 2553].map((n) => specimen(
          `${n} msgs`, rib(conv(rep('c o', Math.max(2, Math.round(n / 2))), { n }))))
      ].flat())));
      return out;
    });

  function rib(c, opts) {
    const o = opts || {};
    const keep = UI.state.fidelity;
    if (o.fidelity) UI.state.fidelity = o.fidelity;
    const wrap = el('div', 'row-meta');
    wrap.style.gridTemplateColumns = '200px';
    const s = el('span', 'ribbon');
    s.innerHTML = UI.ribbon(c, Boolean(o.q));
    wrap.appendChild(s);
    UI.state.fidelity = keep;
    return wrap;
  }

  section('row', 'Row', 'the unit of the list',
    'Two clusters with a gutter: agent, model, size, change and shape on the left; ' +
    'directory, forks and age on the right. The agent badge is <b>icon-only</b> here — ' +
    'forced, not chosen: the list column is 706px and the word cost the 66px that ' +
    '<code>cwd</code> needed to stop collapsing to zero.',
    () => {
      const base = conv('u ' + rep('c:run o', 8) + ' c:change o ' + rep('c:run o', 6) + ' a');
      const withQ = conv('u ' + rep('c o', 6) + ' c* o a ' + rep('c o', 5) + ' a');
      withQ.title = 'ymd() renders UTC instead of the local day';
      const bare = conv(rep('a u', 4), { topics: [], edits: [0, 0], src: 'chatgpt',
        model: 'gpt-5.4', dir: '—', project: null, files: [] });
      bare.title = 'Is a civil day always 86,400 seconds?';

      const rows = el('div', 'list');
      const add = (c, cap, cls) => {
        const r = UI.conversationRow(c);
        if (cls) r.classList.add(cls);
        rows.appendChild(specimen(cap, r));
      };
      add(base, 'three lines — meta, title, topics');
      // The fourth line is gated on `hasQuery()`, which reads state, so a specimen of
      // it has to put the state there. Without this the caption described a row the
      // page was not rendering — a gallery drifting from the app inside one file.
      withQuery(true, () =>
        add(withQ, 'four lines — the best-match line appears only when there is a query'));
      add(bare, 'two lines — no topics matched, no edits, a chat surface with no cwd');

      const sel = UI.conversationRow(base);
      sel.setAttribute('aria-selected', 'true');
      rows.appendChild(specimen('selected', sel));
      const hov = UI.conversationRow(base);
      hov.classList.add('gal-force-hover');
      rows.appendChild(specimen('hover', hov));

      const clock = UI.conversationRow(base, 'clock');
      rows.appendChild(specimen('inside a group — the path cell spends itself on the hour, ' +
        'because the run divider above already names the day', clock));
      return rows;
    });

  section('badges', 'Source marks', 'five sources, three channels',
    'CSS masks tinted from the palette, not brand logos: two of the five ship ' +
    'white-on-transparent and one on an opaque white square, and optical weight ranged ' +
    '17%–70% ink. Masking gives shape, hue and word as three redundant channels — of ' +
    'which the row shows two, since it hides the word.',
    () => {
      const full = el('div', 'gal-row');
      const tiny = el('div', 'gal-row');
      const iconOnly = el('div', 'gal-row row-meta');
      iconOnly.style.gridTemplateColumns = 'repeat(5, 22px)';
      ['claude-code', 'codex', 'chatgpt', 'claude.ai', 'gemini-cli'].forEach((s) => {
        full.innerHTML += sourceBadge(s);
        tiny.innerHTML += sourceBadge(s, 'tiny');
        iconOnly.innerHTML += sourceBadge(s);
      });
      return stack([
        caseOf('full — drawer and group headers', specimen('', full)),
        caseOf('icon only — the row', specimen('', iconOnly)),
        caseOf('tiny — group header source set', specimen('', tiny)),
      ]);
    });

  section('chips', 'Chips', 'topic, file, act, fold',
    'Solid chips are interactive, dashed ones are not — the distinction is load-bearing ' +
    'and easy to read as inconsistency, so it is worth checking here rather than in situ.',
    () => {
      const mk = (html) => { const d = el('div', 'gal-row'); d.innerHTML = html; return d; };
      return stack([
        caseOf('topic — solid, filters; `on` when the filter is applied', specimen('',
          mk('<span class="tchip sm">Rust and cargo<span class="c">72</span></span>' +
             '<span class="tchip sm on">SQLite and FTS5<span class="c">65</span></span>' +
             '<span class="tchip sm dim">Terminal UI<span class="c">0</span></span>' +
             '<span class="tchip sm ghost">mode facet</span>' +
             '<button class="tchip sm more" type="button">+12</button>'))),
        caseOf('file — dashed, scoped to a project, full paths', specimen('',
          mk('<span class="tchip sm ghost"><span class="nm">crates/cs-core/src/search.rs</span>' +
             '<span class="c">415</span></span>' +
             '<span class="tchip sm ghost"><span class="nm">docs/DECISIONS.md</span>' +
             '<span class="c">220</span></span>'))),
        caseOf('act — what the calls were for', specimen('',
          mk('<span class="act run"><i>›</i>run<b>76%</b></span>' +
             '<span class="act change"><i>✎</i>change<b>14%</b></span>' +
             '<span class="act look"><i>◎</i>look<b>10%</b></span>' +
             '<span class="act steer"><i>⚑</i>steer<b>×2</b></span>' +
             '<span class="act sub"><i>⑃</i>subagents<b>37%</b></span>'))),
        caseOf('query tokens — every narrowing, with its own clear', specimen('',
          mk('<span class="tok free"><span class="v">borrow checker</span></span>' +
             '<span class="tok facet"><b>agent:</b><span class="v">codex</span>' +
             '<button class="x" type="button">×</button></span>' +
             '<span class="tok facet"><b>dir:</b><span class="v">projects/chat-search</span>' +
             '<button class="x" type="button">×</button></span>' +
             '<span class="tok facet nogram"><b>topic:</b><span class="v">Rust and cargo</span>' +
             '<button class="x" type="button">×</button></span>'), true)),
      ]);
    });

  section('controls', 'Detail controls', 'two axes, one chip',
    'Visibility and detail are two axes, not three states on one — with a single ' +
    '3-cycle, one of the six transitions always costs two clicks. The chip body cycles ' +
    '<code>off → brief → full</code>; the dot toggles visibility and restores the level ' +
    'that kind last had.',
    () => {
      const grid = el('div', 'fid');
      UI.KINDS.forEach(({ k, label }, i) => {
        const lv = ['expanded', 'collapsed', 'hidden', 'collapsed'][i];
        grid.innerHTML +=
          `<div class="fid-chip${lv === 'hidden' ? '' : ' on'}">` +
          `<button type="button" class="dot" aria-pressed="${lv !== 'hidden'}" style="--k:var(--k-${k})"></button>` +
          `<button type="button" class="lv"><span class="nm">${label}</span>` +
          `<span class="st">${UI.LEVEL_WORD[lv]}</span></button></div>`;
      });
      const all = el('div', 'fid');
      UI.LEVELS.forEach((lv) => {
        all.innerHTML +=
          `<div class="fid-chip${lv === 'hidden' ? '' : ' on'}">` +
          `<button type="button" class="dot" aria-pressed="${lv !== 'hidden'}" style="--k:var(--k-tool)"></button>` +
          `<button type="button" class="lv"><span class="nm">tools</span>` +
          `<span class="st">${UI.LEVEL_WORD[lv]}</span></button></div>`;
      });
      const presets = el('div', 'zoom');
      Object.keys(UI.PRESETS).forEach((z, i) => {
        presets.innerHTML += `<button type="button" data-zoom="${z}" aria-pressed="${i === 0}">${z}</button>`;
      });
      const group = el('div', 'group');
      group.innerHTML = '<span class="group-l">group</span>' +
        UI.GROUPINGS.map((g, i) => `<button type="button" aria-pressed="${i === 1}">${g.label}</button>`).join('');
      return stack([
        caseOf('four kinds, mixed states', specimen('', grid)),
        caseOf('one kind through all three states', specimen('', all)),
        caseOf('presets — `outline` used to be a duplicate of `collapse all`', specimen('', presets)),
        caseOf('grouping — a dimension of the list, not a destination', specimen('', group, true)),
      ]);
    });

  section('groups', 'Group headers', 'four axes, one renderer',
    'Projects and Sittings were both <code>GROUP BY</code> over the same set wearing the ' +
    'costume of destinations. One renderer now; the band beneath differs per axis ' +
    'because each answers a different question about the group.',
    () => {
      const items = [conv('u ' + rep('c:run o', 6) + ' c:change o a'),
                     conv('u ' + rep('c o', 9) + ' a'),
                     conv('u ' + rep('r c o', 5) + ' a')];
      const mkGroup = (over) => Object.assign({
        key: 'k', items, total: 114, label: 'projects/chat-search',
        first: T0 - 5 * 86400000, last: T0,
      }, over);

      const out = el('div', 'list');
      out.appendChild(specimen('project — collapsed, corpus-true count with the sample beside it',
        UI.groupSection(mkGroup({ runs: true, band: null }), false)));
      out.appendChild(specimen('run — cross-tool tag, hours in the date cell',
        UI.groupSection(mkGroup({ label: 'Aug 2', total: 13, crossTool: true,
          when: '15:30 – 20:51', band: null }), false)));
      out.appendChild(specimen('topic — mode facet tagged, because a mode fires everywhere',
        UI.groupSection(mkGroup({ label: 'Prose and editing', total: 96, facet: 'mode',
          band: null }), false)));
      out.appendChild(specimen('the untagged residue — 24% of the corpus matches no seed',
        UI.groupSection(mkGroup({ label: 'no topic matched', total: 87, untagged: true,
          band: null }), false)));
      out.appendChild(specimen('small — folded away below three conversations, and no sparkline',
        UI.groupSection(mkGroup({ label: 'sandbox/chat-history', total: 1,
          items: [items[0]], small: true, band: null }), false)));
      return out;
    });

  section('bands', 'Group bands', 'what the group is',
    'Opened beneath a group header. Each axis explains itself differently: a project by ' +
    'its topics and files, a run by its span and tools, a topic by the seeds that fired.',
    () => {
      const items = [conv('u ' + rep('c:run o', 10) + ' c:change o a?'),
                     conv('u ' + rep('c:look o', 6) + ' a')];
      const p = { path: '/Users/tylercrosse/dev/projects/chat-search',
        name: 'projects/chat-search', n: 114, dirs: 10,
        topics: [{ name: 'Rust and cargo', n: 72, facet: 'subject' },
                 { name: 'SQLite and FTS5', n: 65, facet: 'subject' },
                 { name: 'Git and version control', n: 89, facet: 'mode' }],
        files: [{ path: 'crates/cs-core/src/search.rs', n: 415 },
                { path: 'docs/DECISIONS.md', n: 220 }] };
      return stack([
        caseOf('project', specimen('topics subject-first, files scoped and relative',
          UI.projectBand(p, items))),
        caseOf('run', specimen('', UI.runBand(
          { start: T0 - 5.9 * 3600000, end: T0, items },
          ['claude-code', 'codex'], [p.path]))),
        caseOf('topic', specimen('the seeds that fired, ranked by members reached',
          UI.topicBand({ name: 'Rust and cargo', facet: 'subject', group: 'Systems',
            terms: ['rust 61', 'cargo 44', 'crate 31', 'trait 22'] }, items))),
        caseOf('source', specimen('archetype is ~87% predicted by this',
          UI.sourceBand('claude-code', items))),
        caseOf('untagged', specimen('', UI.untaggedBand(items, 8))),
      ]);
    });

  section('dividers', 'Run dividers', 'labels, not controls',
    'A 12-hour gap, measured: three days — the corpus-wide lineage rule — left ' +
    'chat-search, meety-local and dev/career each as one undivided run. Making these ' +
    'collapsible would imply the rows beneath are optional, and they are the only thing ' +
    'on the screen that is not.',
    () => {
      const d1 = el('div', 'pj-run');
      d1.innerHTML = '<span class="d">Aug 2</span><span class="n">5 conversations</span>';
      const d2 = el('div', 'pj-run');
      d2.innerHTML = '<span class="d">Jul 30 – Jul 31</span><span class="n">18 conversations</span>';
      const more = el('button', 'pj-more');
      more.innerHTML = '<span>94 more</span><span class="go">narrow to this group</span>';
      const tail = el('button', 'pj-tail');
      tail.innerHTML = '<span class="tw">&#9656;</span><span>20 with 3 conversations or fewer</span>' +
        '<span class="n">27</span>';
      return stack([
        caseOf('within a day / spanning two', specimen('', stack([d1, d2]))),
        caseOf('overflow — hands off to a query rather than nesting a scroller',
          specimen('', more)),
        caseOf('the small-project fold — counted in the open', specimen('', tail)),
      ]);
    });

  section('blocks', 'Transcript blocks', 'the preview line',
    'The gutter carries role and kind so text contrast can stay identical for user and ' +
    'assistant — <code>me9.1.1</code>: do not privilege user turns. Tool lines show the ' +
    '<b>argument</b>, not the tool name: <code>Bash</code> told you nothing.',
    () => {
      const c = conv('u a r c:run o c:change o c:look o o!');
      const out = el('div');
      const each = (level) => {
        const d = el('div');
        c.msgs.forEach((m) => {
          const b = UI.block(m, { level });
          if (b) d.appendChild(b);
        });
        return d;
      };
      out.appendChild(caseOf('brief', specimen('', each('collapsed'))));
      out.appendChild(caseOf('full — the whole command, the paragraph, the size',
        specimen('', each('expanded'))));
      const brk = UI.breakRule('6h later');
      out.appendChild(caseOf('boundaries', specimen('a pause interrupts; a compaction cuts',
        stack([brk, UI.cutRule()]))));
      return out;
    });

  section('drawer', 'Drawer header', 'what it is, what it did',
    'One line of acts and one of files touched, both free: the act comes from the tool ' +
    'name, the paths were already mined for the project rollup. Subagents share the ' +
    'acts line because they appear on 57 conversations in the whole corpus.',
    () => {
      const c = conv('u ' + rep('c:run o', 9) + ' ' + rep('c:change o', 3) + ' ' +
        rep('c:look o', 4) + ' a', { sidechain: 0.37,
        files: ['crates/cs-core/src/search.rs', 'docs/DECISIONS.md', 'crates/cs/src/main.rs',
                'crates/cs-import/src/claude_code.rs'] });
      const d = el('div');
      d.innerHTML = UI.workSummary(c);
      d.querySelectorAll('.pv-work .tchip .nm').forEach((n, i) => {
        if (c.files[i]) n.textContent = UI.elidePath(c.files[i], 30);
      });
      return caseOf('work summary', specimen('', d));
    });

  section('empty', 'Empty states', 'the first ones in the prototype',
    'Library is mostly empty and says so — that is the true state of an app where ' +
    'nothing has been authored yet.',
    () => {
      const mk = (b, s) => {
        const e = el('div', 'empty');
        e.innerHTML = '<b></b><span></span>';
        e.querySelector('b').textContent = b;
        e.querySelector('span').textContent = s;
        return e;
      };
      const lib = el('div', 'library');
      lib.appendChild(mk('No collections yet',
        'A collection is stored as matches(query) + pinned − excluded, so it keeps ' +
        'answering after a reindex instead of freezing one answer.'));
      return stack([
        caseOf('library', specimen('', lib, true)),
        caseOf('no rows', specimen('', mk('Nothing to group',
          'No conversation in this selection carries the column this axis groups by.'), true)),
      ]);
    });

  section('colour', 'Colour', 'measured, not picked',
    'Each swatch prints its measured contrast against the surface it is drawn on. The ' +
    'kind ramp is against <code>--map-bg</code>; the ink tiers against <code>--bg</code>. ' +
    'Every number here is computed live from the stylesheet, so it cannot go stale.',
    () => {
      const lum = (c) => c.match(/[\d.]+/g).slice(0, 3).map(Number)
        .map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; })
        .reduce((a, x, i) => a + x * [0.2126, 0.7152, 0.0722][i], 0);
      const ratio = (a, b) => {
        const [x, y] = [lum(a), lum(b)].sort((m, n) => n - m);
        return ((x + 0.05) / (y + 0.05)).toFixed(2);
      };
      const resolve = (v) => {
        const d = el('div');
        d.style.color = v;
        document.body.appendChild(d);
        const out = getComputedStyle(d).color;
        d.remove();
        return out;
      };
      const swatches = (names, againstVar, label) => {
        const g = el('div', 'gal-grid');
        const bg = resolve(`var(${againstVar})`);
        names.forEach((n) => {
          const fg = resolve(`var(${n})`);
          const s = el('div', 'gal-frame');
          s.style.display = 'flex';
          s.style.alignItems = 'center';
          s.style.gap = '10px';
          const sw = el('div');
          sw.style.cssText = `width:30px;height:30px;border-radius:5px;flex:none;background:var(${n})`;
          const t = el('div');
          t.innerHTML = `<div style="font-family:var(--mono);font-size:10px">${n}</div>` +
            `<div style="font-family:var(--mono);font-size:9px;color:var(--ink-3)">` +
            `${ratio(fg, bg)} : 1 vs ${label}</div>`;
          s.appendChild(sw);
          s.appendChild(t);
          g.appendChild(s);
        });
        return g;
      };
      return stack([
        caseOf('kind ramp — an even ~1.8× per step', swatches(
          ['--k-agent', '--k-user', '--k-reason', '--k-tool'], '--map-bg', 'track')),
        caseOf('act sub-shades — narrow on purpose, all below reasoning',
          swatches(['--act-change', '--act-run', '--act-look'], '--map-bg', 'track')),
        caseOf('ink tiers — --ink-3 carries dates, counts, labels and the footer',
          swatches(['--ink', '--ink-2', '--ink-3'], '--bg', 'bg')),
        caseOf('reserved — amber is match, red is failure, teal is selection',
          swatches(['--hit', '--err', '--sel', '--ok'], '--bg', 'bg')),
      ]);
    });

  /* ---------------------------------------------------------------- boot -- */

  function build(root) {
    SECTIONS.forEach((s) => {
      const sec = el('section', 'gal-sec');
      sec.id = s.id;
      const h = el('div', 'gal-h');
      h.innerHTML = '<h2></h2><span></span>';
      h.querySelector('h2').textContent = s.title;
      h.querySelector('span').textContent = s.meta;
      sec.appendChild(h);
      if (s.note) {
        const n = el('p', 'gal-note');
        n.innerHTML = s.note;
        sec.appendChild(n);
      }
      try {
        sec.appendChild(s.build());
      } catch (e) {
        const err = el('div', 'empty');
        err.innerHTML = '<b>This section failed to render</b>';
        err.appendChild(document.createTextNode(String(e && e.message)));
        sec.appendChild(err);
      }
      root.appendChild(sec);
    });
  }

  /* The fixtures, exported for the same reason window.CS_UI exists: directions.html
     shows the row and the ribbon under every direction, and building it a second set of
     conversations would let the two pages disagree about what a specimen is. A shape
     written as `u c o c* o a` is also the most compact way anyone has found to say
     what a ribbon is being asked to draw. */
  window.CS_FIXTURES = { msg, conv, rep, specimen, caseOf, stack, rib, withQuery };

  // Everything below builds the gallery's own page, so it only runs on it. Without the
  // guard, loading this file for its fixtures throws on the first getElementById.
  if (!$('gal-body')) return;

  const params = new URLSearchParams(location.search);
  const pane = params.get('pane');
  if (pane === 'light') document.documentElement.classList.add('light');
  if (pane) {
    document.querySelector('.gal-top').hidden = true;
    document.querySelector('.gal-nav').hidden = true;
  }

  build($('gal-body'));

  const nav = $('gal-nav');
  SECTIONS.forEach((s) => {
    const a = el('a');
    a.href = '#' + s.id;
    a.textContent = s.title;
    nav.appendChild(a);
  });
  $('gal-count').textContent =
    `${SECTIONS.length} sections · ${document.querySelectorAll('.gal-item').length} specimens · ` +
    'styled entirely by styles.css';

  $('gal-theme').addEventListener('click', (e) => {
    const light = document.documentElement.classList.toggle('light');
    e.currentTarget.textContent = light ? 'dark' : 'light';
  });

  /* Two iframes rather than a scoped class: the theme lives on `:root`, so a second
     copy needs a second document to be a real test rather than an approximation. */
  $('gal-both').addEventListener('click', (e) => {
    const on = e.currentTarget.getAttribute('aria-pressed') === 'true';
    e.currentTarget.setAttribute('aria-pressed', on ? 'false' : 'true');
    const body = $('gal-body');
    body.textContent = '';
    if (on) {
      build(body);
      return;
    }
    const pair = el('div', 'gal-pair');
    ['dark', 'light'].forEach((t) => {
      const col = el('div');
      const h = el('div', 'gal-pane-h');
      h.textContent = t;
      const f = el('iframe');
      f.src = `gallery.html?pane=${t}`;
      f.style.cssText = 'width:100%;height:78vh;border:1px solid var(--rule);border-radius:8px';
      col.appendChild(h);
      col.appendChild(f);
      pair.appendChild(col);
    });
    body.appendChild(pair);
  });
})();
