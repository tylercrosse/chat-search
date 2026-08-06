/* chat-search — interface prototype.
 *
 * Clickable with canned data: rows, facets, views, cluster proposals and the minimap
 * all respond. The query box is display-only — but facet clicks still rewrite its
 * text, because that one-source-of-truth rule is the point being demonstrated.
 *
 * Plain script, no modules, no build. Depends on data.js loading first.
 */
(function () {
  'use strict';

  const $ = (id) => document.getElementById(id);
  const el = (tag, cls) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    return n;
  };
  const frag = () => document.createDocumentFragment();

  /* ===================================================== state ============ */

  const state = {
    view: 'search',
    // One filter state. `me9.16` closed the same defect in the TUI — App.source held a
    // selection beside the query text and parsed() reconciled them on every keystroke —
    // and the fix there was that a chip click rewrites the query. This prototype had
    // drifted back: topics lived in a Set and the timeline range in a tuple, and neither
    // appeared in the query line, so the list could be narrowed with nothing on screen
    // saying so. Everything that narrows the list now lives here and is drawn.
    //
    // `agents` and `dirs` are lists because cs_core::query's Facet::{Agent,Dir} take
    // comma lists and union on repeat. `topics` and `range` have NO grammar in
    // cs_core::query — they are drawn as tokens and marked as ungrammatical rather than
    // given invented syntax. See chat-search-4ar.10.
    query: { free: '', agents: [], date: '', dirs: [], topics: [], range: null },
    // Grouping is a dimension of the one list, not a place. Projects and Sittings were
    // both GROUP BY over the same set, wearing the costume of destinations.
    group: 'none',              // none | project | run | topic | source
    selected: 'c1',
    // Per-kind fidelity is the model. The three zoom names below are presets that
    // set all four knobs at once; touching any knob individually reads as 'custom'.
    zoom: 'segments',
    fidelity: { user: 'expanded', agent: 'collapsed', reason: 'hidden', tool: 'hidden' },
    // Per-message overrides, keyed conv:index. me9.1.1 asks for expand-all/collapse-all
    // *and* per-message toggling; the kind controls are the first, this is the second.
    overrides: new Map(),
    openSegments: new Set(),
    topics: new Set(),          // topic chips, ANDed
    range: null,                // [lo, hi] from the timeline scrubber
    timeline: true,             // bottom drawer open
    zoomTouched: false,
    showOffPath: false,
    annotations: false,
    openCollection: null,
    dismissed: new Set(),
    accepted: new Set(),        // proposals promoted to collections
    expanded: new Set(),        // groups open in the accordion, keyed `group:key`
    uncapped: new Set(),        // groups showing past the 20-row cap
    smallShown: false,          // the tail of one- and two-conversation projects
    railProjects: false,        // the project rail past its first eight
    pvTopics: false,            // the drawer's topic chips past the first three
  };

  /* The list is one list. These are the axes it can be cut along, and every one of them
     is a column that already exists — no new machinery, and each says what it costs. */
  const GROUPINGS = [
    { k: 'none', label: 'none', note: 'ranked, ungrouped' },
    { k: 'project', label: 'project', note: 'cwd · 34% of conversations, 92% of messages' },
    { k: 'run', label: 'run', note: 'ended_at · 100% coverage' },
    { k: 'topic', label: 'topic', note: 'seeded keywords · a conversation can be in several' },
    { k: 'source', label: 'source', note: 'archetype is ~87% predicted by it' },
  ];

  /* With real data, nothing has been accepted yet — an empty Collections shelf and a
   * full Proposed shelf is the honest starting state, and it is the accept gesture that
   * needs exercising. The fixtures below stand in when no export is present. */
  const collections = REAL && REAL.topics ? [] : [
    { id: 'k1', name: 'Retention & capture', query: 'retention OR archive OR scanner',
      pinned: ['c15'], excluded: [] },
    { id: 'k2', name: 'Dates and timezones', query: 'utc OR timezone OR civil date OR local day',
      pinned: [], excluded: [] },
    { id: 'k3', name: 'Ranking quality', query: 'bm25 OR stemming OR eval OR prefix',
      pinned: ['c8'], excluded: ['c3'] },
  ];

  /* A cluster is a proposal. Being wrong costs one dismissed suggestion.
   * From the export these are *seeded* topics — membership by keyword, explained by the
   * seeds that actually hit — rather than discovered clusters. */
  let proposals = REAL && REAL.topics
    ? REAL.topics.map((t, i) => ({
        id: 'p' + i, name: t.name, members: t.members,
        terms: t.terms, seed: t.seed, seeded: true,
      }))
    : [
    { id: 'p1', name: 'SQLite / FTS5 internals', query: 'fts5 OR sqlite OR rowid OR contentless',
      terms: ['fts5', 'contentless', 'rowid', 'sqlite-vec', 'external-content'], distance: 0.31 },
    { id: 'p2', name: 'Importer edge cases', query: 'resume OR reflink OR importer OR transcript',
      terms: ['resume id', 'collide', 'reflink', 'purity', 'apfs'], distance: 0.38 },
    { id: 'p3', name: 'The message DAG', query: 'dag OR branch OR parent OR fork OR grouping',
      terms: ['parent_id', 'on_head_path', 'edited away', 'fork'], distance: 0.44 },
  ];

  /* ===================================================== helpers ========== */

  /* Middle elision preserving the leaf. CSS text-overflow can only tail-elide, which
   * discards the discriminating token: ~/dev/projects/personal-site would become
   * ~/dev/projects/pe… — exactly the part that identified it. TUI-DESIGN §2. */
  function elidePath(path, budget) {
    if (path.length <= budget || path === '—') return path;
    const parts = path.split('/');
    if (parts.length < 3) return path.slice(0, budget - 1) + '…';
    const leaf = parts[parts.length - 1];
    for (let k = parts.length - 2; k >= 1; k--) {
      const c = `${parts.slice(0, k).join('/')}/…/${leaf}`;
      if (c.length <= budget) return c;
    }
    return `…/${leaf}`;
  }
  // Sized to the grid track, not guessed. At 20 the string was 120px in a 108px cell, so
  // CSS tail-elided it to `/Users/…/chat-sear` — discarding the leaf, which is the whole
  // failure elidePath() exists to prevent. Reintroducing it by mismatched budget is
  // exactly how that bug comes back.
  const DIR_CHARS = 17;

  const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
                  'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

  /* Rendered once, here, and read everywhere else. Three clients each deriving the day
   * themselves is what produced the local-date bug; the same rule applies to a
   * prototype with three views in one file. */
  function dayLabel(ts, withYear) {
    if (!ts) return '—';
    const d = new Date(ts);
    const y = withYear && d.getFullYear() !== new Date().getFullYear()
      ? ` ’${String(d.getFullYear()).slice(2)}` : '';
    return `${MONTHS[d.getMonth()]} ${d.getDate()}${y}`;
  }

  const DAY = 86400000;

  /* One map renderer, three scales: row strip, sitting card, big minimap. The TUI
   * spec's rule that the density strip and outline mode are the same data at two
   * resolutions, extended rather than duplicated. */
  function mapBands(msgs, width) {
    const n = msgs.length;
    const cols = Math.min(n, width);
    const cw = width / cols;
    const buckets = Array.from({ length: cols }, () =>
      ({ p: 0, r: 0, t: 0, hit: false, cut: false, len: 0 }));

    msgs.forEach((m, i) => {
      const b = buckets[Math.min(cols - 1, Math.floor((i / n) * cols))];
      if (m.kind === 'prose') b.p++;
      else if (m.kind === 'reasoning') b.r++;
      else b.t++;
      b.len = Math.max(b.len, Math.log10(m.len));
      if (m.match) b.hit = true;
      if (m.compacted) b.cut = true;
    });

    return buckets
      .map((b, i) => {
        const kind = b.p ? 'prose' : b.r ? 'reason' : 'tool';
        const h = Math.max(2, Math.min(12, (b.len / 4.8) * 12));
        const x = (i * cw).toFixed(2);
        const w = Math.max(0.8, cw - 0.35).toFixed(2);
        return (
          `<i class="${kind}" style="left:${x}px;width:${w}px;height:${h.toFixed(1)}px"></i>` +
          (b.hit ? `<i class="hit" style="left:${x}px;width:${Math.max(1.5, cw).toFixed(2)}px"></i>` : '') +
          // The minimap is how you cross a 1,300-message conversation, so the one
          // boundary that changes what the agent knew belongs on it.
          (b.cut ? `<i class="cut" style="left:${x}px"></i>` : '')
        );
      })
      .join('');
  }

  function matchesQuery(conv, query) {
    return query
      .split(/\s+OR\s+/i)
      .some((term) => conv.title.toLowerCase().includes(term.trim().toLowerCase()));
  }

  /* The rule, evaluated. Returned in parts so the card can show the arithmetic
   * rather than only the total. */
  /* Tolerates a def with no pins or exclusions: that is exactly a cluster proposal,
   * which is pure query until someone accepts it and starts curating. */
  function resolve(def) {
    const pins = def.pinned || [];
    const cuts = def.excluded || [];
    // A seeded topic arrives with its membership already decided by the export; only
    // fixture definitions have to be matched here.
    const matched = def.members
      ? CONVERSATIONS.filter((c) => def.members.includes(c.id))
      : CONVERSATIONS.filter((c) => matchesQuery(c, def.query));
    const matchedIds = new Set(matched.map((c) => c.id));
    const netPinned = pins.filter((id) => !matchedIds.has(id));
    const members = CONVERSATIONS.filter(
      (c) => (matchedIds.has(c.id) || pins.includes(c.id)) && !cuts.includes(c.id)
    );
    return { members, nMatched: matched.length, nPinned: netPinned.length, nExcluded: cuts.length };
  }

  function composition(convs) {
    const by = {};
    convs.forEach((c) => { by[c.src] = (by[c.src] || 0) + 1; });
    return Object.entries(by).sort((a, b) => b[1] - a[1]);
  }

  function compositionBar(convs) {
    const parts = composition(convs);
    const total = convs.length || 1;
    const bar = parts
      .map(([src, n]) => `<i style="background:${SOURCES[src].hue};width:${(n / total) * 100}%"></i>`)
      .join('');
    const key = parts
      .map(([src, n]) =>
        `<span style="color:${SOURCES[src].hue}"><i class="ico" style="--ico:${SOURCES[src].ico}"></i>${src} ${n}</span>`)
      .join('');
    return { bar, key, tools: parts.length };
  }

  /* ===================================================== chrome =========== */

  /* Every narrowing, drawn. A filter the list is applying but the query line is not
     showing is the exact defect `me9.16` closed in the TUI, and the reason TUI-DESIGN §5
     insists on one state: you cannot reason about thirty rows when you cannot see why
     there are thirty. Tokens carry a × because a filter you can apply and not remove is
     half a control. */
  function drawQuery() {
    const q = $('qbox');
    q.textContent = '';

    const push = (cls, label, value, clear) => {
      const s = el('span', 'tok ' + cls);
      s.innerHTML = (label ? `<b>${label}</b>` : '') + '<span class="v"></span>' +
        (clear ? '<button class="x" type="button" aria-label="remove">×</button>' : '');
      s.querySelector('.v').textContent = value;
      if (clear) s.querySelector('.x').addEventListener('click', () => { clear(); render(); });
      q.appendChild(s);
    };

    const Q = state.query;
    if (Q.free) push('free', '', Q.free, () => { Q.free = ''; });
    if (Q.agents.length) push('facet', 'agent:', Q.agents.join(','), () => { Q.agents = []; });
    if (Q.date) push('facet', 'date:', Q.date, () => { Q.date = ''; });
    if (Q.dirs.length) {
      const names = Q.dirs.map((d) => (PROJECT_BY_PATH.get(d) || { name: d }).name);
      push('facet', 'dir:', names.join(','), () => { Q.dirs = []; });
    }
    // No grammar for these in cs_core::query — Facet is {Agent, Dir, Date} and nothing
    // else. Drawn as tokens because they narrow the list, marked because they cannot yet
    // be typed, copied out, or replayed. Filed as chat-search-4ar.10.
    Q.topics.forEach((t) =>
      push('facet nogram', 'topic:', t, () => { Q.topics = Q.topics.filter((x) => x !== t); }));
    if (Q.range) {
      push('facet nogram', 'when:',
        `${dayLabel(Q.range[0], true)} – ${dayLabel(Q.range[1], true)}`,
        () => { Q.range = null; });
    }
    q.appendChild(el('span', 'caret'));
  }

  /* Sits beside the query it modifies, not up in the title bar with the views, because
     grouping is a property of this list rather than a place to go. */
  function drawGroupControl() {
    const g = $('group');
    g.textContent = '';
    g.hidden = state.view !== 'search';
    if (g.hidden) return;
    const lbl = el('span', 'group-l');
    lbl.textContent = 'group';
    g.appendChild(lbl);
    GROUPINGS.forEach((x) => {
      const b = el('button');
      b.type = 'button';
      b.textContent = x.label;
      b.title = x.note;
      b.setAttribute('aria-pressed', state.group === x.k ? 'true' : 'false');
      b.addEventListener('click', () => {
        if (state.group === x.k) return;
        state.group = x.k;
        // Every axis starts folded, including one you have opened before. Returning to
        // `project` and finding it in whatever shape you left it is worse than it
        // sounds: the groups are ranked, so the set you left open is rarely the set at
        // the top now.
        state.expanded.clear();
        state.uncapped.clear();
        render();
      });
      g.appendChild(b);
    });
  }

  function drawStat() {
    const shown = visible().length;
    const corpus = REAL ? REAL.corpus : null;
    const hit = CONVERSATIONS.filter((c) => c.bestMatch).length;
    $('stat').innerHTML = corpus
      ? `<b>${shown}</b> shown &nbsp;·&nbsp; <b>${hit}</b> with a match in this sample ` +
        `&nbsp;·&nbsp; ${corpus.conversations.toLocaleString()} indexed`
      : `<b>${shown}</b> of <b>655</b> matched &nbsp;·&nbsp; 2,963 indexed &nbsp;·&nbsp; 4.1 ms`;
  }

  function drawFooter() {
    const keys = '<span><kbd>↑↓</kbd> move</span><span><kbd>⏎</kbd> open</span>' +
      '<span><kbd>⌥⏎</kbd> destination</span>';
    const bits = state.view === 'library'
      ? '<span><kbd>⏎</kbd> open</span><span>authored, not derived — survives a reindex</span>'
      : state.group === 'none' ? keys
      : keys + '<span><kbd>→</kbd> expand</span>' +
        '<span>group counts are corpus-wide; row counts are this sample</span>';
    $('fbar').innerHTML =
      bits + '<span class="sp">index 293 MB · importer v7 · archived 4 min ago</span>';
  }

  /* ===================================================== sidebar ========== */

  function head(label, meta) {
    const h = el('div', 'side-h');
    h.innerHTML = `<span>${label}</span><span>${meta || ''}</span>`;
    return h;
  }

  function plainRow(glyph, text, count, onClick) {
    const b = el('button', 'srow');
    b.type = 'button';
    b.innerHTML = `<span class="glyph">${glyph}</span><span class="t"></span><span class="n">${count.toLocaleString()}</span>`;
    b.querySelector('.t').textContent = text;
    if (onClick) b.addEventListener('click', onClick);
    return b;
  }

  /* Topics, grouped and split by facet. Subject and mode are different axes: "Prose
     and editing" (mode) and "Technical writing and docs" (subject) read as duplicates in
     a flat list and share only 0.13 Jaccard on the corpus. Separating the axes is what
     stops them competing for the same slot. */
  function drawTopics(f) {
    const topics = (REAL && REAL.topics) || [];
    if (!topics.length) return;
    const base = visible();

    ['subject', 'mode'].forEach((facet) => {
      const inFacet = topics.filter((t) => t.facet === facet);
      if (!inFacet.length) return;
      f.appendChild(head(facet === 'subject' ? 'Subject' : 'Mode',
                         facet === 'subject' ? 'what about' : 'how'));
      const groups = {};
      inFacet.forEach((t) => (groups[t.group] = groups[t.group] || []).push(t));

      Object.entries(groups).forEach(([g, list]) => {
        const gh = el('div', 'tgroup');
        gh.innerHTML = `<span class="gname">${g}</span>`;
        list.sort((x, y) => y.members.length - x.members.length).forEach((t) => {
          // Count against what is already filtered, so a chip never promises rows the
          // other facets have removed.
          const n = base.filter((c) => t.members.includes(c.id)).length;
          const on = hasTopic(t.name);
          const b = el('button', 'tchip' + (on ? ' on' : '') + (n ? '' : ' dim'));
          b.type = 'button';
          b.setAttribute('aria-pressed', on ? 'true' : 'false');
          b.innerHTML = `<span class="nm"></span><span class="c">${n}</span>`;
          b.querySelector('.nm').textContent = t.name;
          b.addEventListener('click', () => {
            toggleTopic(t.name);
            render();
          });
          gh.appendChild(b);
        });
        f.appendChild(gh);
      });
    });
  }

  /* Ordered by coverage, not by how interesting the facet is. Topics led, took two
     thirds of the height, and pushed the two axes that reach 100% of the corpus below
     the fold. `ended_at` 100% · source 100% · topic 76% · cwd 34% is the order.

     No clear-all rows any more: every filter now carries its own × in the query line,
     which is the one place all of them are visible at once. */
  function drawSidebar() {
    const side = $('side');
    side.textContent = '';
    const f = frag();

    f.appendChild(head('When', 'ended_at · every conversation'));
    WHEN_ROWS.forEach((w) => {
      const r = plainRow('·', w.t, w.n, () => toggleDate(w.token));
      r.setAttribute('aria-pressed', state.query.date === w.token ? 'true' : 'false');
      f.appendChild(r);
    });

    const sh = head('Sources', 'agent: · config ∪ index');
    sh.setAttribute('data-anno', 'sources');
    f.appendChild(sh);
    SOURCE_ROWS.forEach((s2) => {
      const cls = 'srow' + (s2.state === 'zero' ? ' zero' : s2.state === 'unconfigured' ? ' unconfigured' : '');
      const b = el('button', cls);
      b.type = 'button';
      b.setAttribute('aria-pressed', state.query.agents.includes(s2.k) ? 'true' : 'false');
      const src = SOURCES[s2.k];
      b.innerHTML =
        (src && src.ico ? `<i class="ico" style="--ico:${src.ico};color:${src.hue}"></i>` : '<span class="glyph">·</span>') +
        '<span class="t"></span>' +
        (s2.state === 'zero' ? '<span class="warn">!</span>' : '') +
        `<span class="n">${s2.n === null ? '—' : s2.n.toLocaleString()}</span>`;
      b.querySelector('.t').textContent = s2.k;
      if (s2.state === 'unconfigured') b.disabled = true;
      else b.addEventListener('click', () => { toggleAgent(s2.k); render(); });
      f.appendChild(b);
    });

    drawProjects(f);
    drawTopics(f);
    side.appendChild(f);
  }

  /* `dir:` was the one facet with no rail section — it could only be acquired by
     grouping by project and clicking through, which made it the only filter you could
     not reach directly. Counts are corpus-true, so this is also the only place the
     whole project list is visible. */
  function drawProjects(f) {
    if (!PROJECTS.length) return;
    const shown = PROJECTS.filter((p) => p.sampled > 0);
    if (!shown.length) return;

    f.appendChild(head('Projects', `dir: · ${PROJECTS.length} indexed`));
    const open = state.railProjects;
    const list = open ? shown : shown.slice(0, 8);
    list.forEach((p) => {
      const on = state.query.dirs.includes(p.path);
      const b = el('button', 'srow');
      b.type = 'button';
      b.setAttribute('aria-pressed', on ? 'true' : 'false');
      b.innerHTML = '<span class="glyph">▸</span><span class="t"></span>' +
                    `<span class="n">${p.n}</span>`;
      b.querySelector('.t').textContent = p.name;
      b.addEventListener('click', () => { toggleDir(p.path); keepSelection(); render(); });
      f.appendChild(b);
    });
    if (shown.length > 8) {
      const more = el('button', 'srow muted');
      more.type = 'button';
      more.innerHTML = `<span class="glyph">${open ? '▴' : '▾'}</span><span class="t">` +
        `${open ? 'fewer' : `${shown.length - 8} more`}</span>`;
      more.addEventListener('click', () => { state.railProjects = !open; render(); });
      f.appendChild(more);
    }
  }

  /* A facet click rewrites the query. There is no second filter state to reconcile —
   * TUI-DESIGN §5, where fast-resume needed six methods to do so, and `me9.16`, which
   * closed the same defect in the shipped TUI. These mutate and do not render, so a
   * caller can set several at once (narrowing to a group changes both the facet and the
   * grouping) and still repaint exactly once. */
  function toggleAgent(k) {
    const i = state.query.agents.indexOf(k);
    if (i === -1) state.query.agents.push(k);
    else state.query.agents.splice(i, 1);
    keepSelection();
  }

  /* A filter that removes the selected conversation leaves the drawer showing something
     the list no longer contains. */
  function keepSelection() {
    if (state.view !== 'search') state.view = 'search';
    const v = visible();
    if (!v.some((c) => c.id === state.selected) && v.length) state.selected = v[0].id;
  }

  /* '3d' | '2w' | '5mo' → days, so the date facet actually selects rather than just
   * decorating the query bar. */
  function ageDays(age) {
    const n = parseInt(age, 10);
    if (age.endsWith('mo')) return n * 30;
    if (age.endsWith('w')) return n * 7;
    if (age.endsWith('d')) return n;
    return n * 365;
  }

  const DATE_WINDOWS = {
    today: (d) => d <= 1,
    week: (d) => d <= 7,
    month: (d) => d <= 31,
    '>1mo': (d) => d > 31,
  };

  /* Same rule as the source facets: the click edits the query text, and the bar
   * redraws from it. Nothing holds a selection beside the query. */
  function toggleDate(token) {
    state.query.date = state.query.date === token ? '' : token;
    if (state.view !== 'search') state.view = 'search';
    const v = visible();
    if (!v.some((c) => c.id === state.selected) && v.length) state.selected = v[0].id;
    render();
  }

  /* With real data the matches are not baked in — recompute against the free text
     before filtering, so ticks and the best-match line follow what is typed. */
  function refreshMatches() {
    if (typeof applyQuery === 'function' && REAL) applyQuery(CONVERSATIONS, state.query.free);
  }

  const topicMembers = (name) => {
    const t = (REAL && REAL.topics || []).find((x) => x.name === name);
    return t ? t.members : [];
  };

  /* Toggling goes through one function for the same reason cs_core::query exposes
     `toggling` rather than letting each caller edit the string: five call sites used to
     reach into the state directly, which is how the second source of truth gets in. */
  const hasTopic = (name) => state.query.topics.includes(name);
  const toggleTopic = (name) => {
    state.query.topics = hasTopic(name)
      ? state.query.topics.filter((t) => t !== name)
      : state.query.topics.concat(name);
  };
  const toggleDir = (path) => {
    state.query.dirs = state.query.dirs.includes(path)
      ? state.query.dirs.filter((d) => d !== path)
      : state.query.dirs.concat(path);
  };

  /* Every filter narrows the same set: source chips, the date facet, topic chips and
     the timeline range. The timeline drawer draws whatever survives the others, so the
     two stay consistent instead of describing different things. */
  const visible = (ignoreRange) => {
    const q = state.query;
    return CONVERSATIONS.filter((c) => {
      // A facet with several values unions, matching Facet::parse_selection.
      if (q.agents.length && !q.agents.includes(c.src)) return false;
      if (q.dirs.length && !q.dirs.includes(c.project)) return false;
      const win = DATE_WINDOWS[q.date];
      if (win && !win(ageDays(c.age))) return false;
      // Topics intersect rather than union: two chips mean "both", which is the only
      // reading that makes overlapping tags useful for narrowing.
      for (const t of q.topics) if (!topicMembers(t).includes(c.id)) return false;
      if (!ignoreRange && q.range && c.endedAt) {
        if (c.endedAt < q.range[0] || c.endedAt > q.range[1]) return false;
      }
      return true;
    });
  };

  /* ================================================ view: search ========== */

  // Matches the grid track. When these drifted apart the track overflowed its cell and
  // spilled into the gutter, which is the one column whose whole job is to be empty.
  const RIBBON_W = 200;
  const FOUR_HOURS = 4 * 3600000;

  /* What the ribbon is an axis of: the messages a reader draws, which is not all of them.
     A successful tool result is not drawn — the call implies it — so a 937-message
     conversation is 563 positions long, and the row's `n msgs` is the wrong denominator
     for anything on this graphic. The rule is cs_core::blocks::drawn, carried per message
     by export.py; the fixtures predate it and are all drawn. */
  const drawnMsgs = (conv) => conv.msgs.filter((m) => m.drawn !== false);

  /* The ribbon used to draw 236px whether the conversation held 10 messages or 2,553,
     so the one mark that could carry scale was the one mark that refused to. The track
     now takes a share of the cell by log length: linear would render everything under
     ~200 messages as a stub, since the corpus spans 4 to 2,553. Floor at 14% so a short
     conversation is still a shape rather than a dot; the cell keeps its full width, so
     the columns either side stay on the grid. */
  const LONGEST = Math.max(120, ...CONVERSATIONS.map((c) => drawnMsgs(c).length));
  const ribbonWidth = (n) =>
    RIBBON_W * Math.max(0.14, Math.min(1, Math.log(Math.max(n, 2)) / Math.log(LONGEST)));

  /* cs's four band names in the class names the four stylesheets spell. `reasoning` is
     the only one that differs, and only because `--k-reason` is already the token. */
  const BAND_CLASS = { user: 'user', agent: 'agent', reasoning: 'reason', tool: 'tool' };

  /* Four categories, matching the four fidelity knobs and the four ribbon colours.
     Real conversations arrive with the band already decided (cs_core::blocks::band, via
     `cs show --json` and the export); the derivation below is what the invented fixtures
     fall back to, and it is the only copy of that rule left in the prototype. */
  const bandClass = (m) =>
    BAND_CLASS[m.band] ||
    (m.kind === 'prose' ? (m.role === 'user' ? 'user' : 'agent')
     : m.kind === 'reasoning' ? 'reason'
     : 'tool');

  const KINDS = [
    { k: 'user',   label: 'you' },
    { k: 'agent',  label: 'agent' },
    { k: 'reason', label: 'reasoning' },
    { k: 'tool',   label: 'tools' },
  ];

  const LEVELS = ['hidden', 'collapsed', 'expanded'];
  /* The model keeps the names TUI-DESIGN §8 uses; the chip shows shorter words, because
     "collapsed" does not fit a 10px chip next to its kind and "off" says the same thing
     faster. */
  const LEVEL_WORD = { hidden: 'off', collapsed: 'brief', expanded: 'full' };

  /* Four presets, no all-buttons. There used to be three presets plus `expand all` and
     `collapse all`, and two of those five were the same command — `outline` sets every
     kind to collapsed and so did `collapse all` — while `full` was not full (it left
     reasoning and tools collapsed) and `expand all` was. Naming a preset for what it
     does and letting `everything` mean everything removes both the duplicate and the
     lie. */
  const PRESETS = {
    segments:   { user: 'expanded',  agent: 'collapsed', reason: 'hidden',    tool: 'hidden' },
    outline:    { user: 'collapsed', agent: 'collapsed', reason: 'collapsed', tool: 'collapsed' },
    read:       { user: 'expanded',  agent: 'expanded',  reason: 'collapsed', tool: 'collapsed' },
    everything: { user: 'expanded',  agent: 'expanded',  reason: 'expanded',  tool: 'expanded' },
  };

  /* Visibility and detail are two axes, not three states on one. Hiding a kind is
     "is it on screen"; brief/full is "how much of it". Conflating them is why every
     arrangement of this control has felt wrong, including the segmented one it replaces:
     with a single 3-cycle, one of the six transitions always costs two clicks.

     So the chip carries both. Its body cycles off → brief → full → off, which is the
     path you actually walk (peek at the tools, then read them, then put them away) —
     the earlier note rejecting a cycle optimised the rare transition. Its dot toggles
     visibility outright and restores whatever detail level the kind last had, which is
     the escape a pure cycle cannot give you. */
  const lastLevel = { user: 'expanded', agent: 'collapsed', reason: 'collapsed', tool: 'collapsed' };

  function cycleKind(k, back) {
    const i = LEVELS.indexOf(state.fidelity[k]);
    const next = LEVELS[(i + (back ? LEVELS.length - 1 : 1)) % LEVELS.length];
    state.fidelity[k] = next;
    if (next !== 'hidden') lastLevel[k] = next;
  }

  function toggleKind(k) {
    if (state.fidelity[k] === 'hidden') state.fidelity[k] = lastLevel[k];
    else { lastLevel[k] = state.fidelity[k]; state.fidelity[k] = 'hidden'; }
  }

  const ovrKey = (m) => state.selected + ':' + m.i;
  const levelOf = (m) =>
    state.overrides.get(ovrKey(m)) || state.fidelity[bandClass(m)];

  /* Segments mode summarises a steer plus the run it caused. On a conversational
   * thread there is no run: real ChatGPT conversations rendered "→ 1 messages" after
   * every single turn, doubling the line count to say nothing. The default has to
   * follow the archetype, and prose fraction already gives it. */
  function defaultZoomFor(conv) {
    const prose = conv.msgs.filter((m) => m.kind === 'prose').length / (conv.msgs.length || 1);
    return Object.assign({}, prose > 0.5 ? PRESETS.full : PRESETS.segments);
  }

  function selectConversation(id) {
    state.selected = id;
    state.overrides.clear();
    const c = CONVERSATIONS.find((x) => x.id === id);
    if (c && !state.zoomTouched) state.fidelity = defaultZoomFor(c);
    render();
  }
  const isShown = (m) => levelOf(m) !== 'hidden' && (m.onPath || state.showOffPath);

  /* Which preset, if any, the current knobs match. */
  function activePreset() {
    return Object.keys(PRESETS).find((p) =>
      KINDS.every(({ k }) => PRESETS[p][k] === state.fidelity[k])
    ) || null;
  }

  /* One graphic, two channels. The body is shape and is query-independent; the ticks
   * are matches and vanish with no query. Notches mark >4h pauses and resumes. */
  /* Run-length encode consecutive same-kind messages into blocks. Per-message bands
   * at this width are 1.6px of near-identical grey — the structure only appears once
   * a 30-message tool run is drawn as one block. */
  /* Runs break on act as well as on kind, so a patch landing inside forty exec calls
     shows as its own band instead of being swallowed by the run around it. A tool_result
     inherits the act of the call it answers — they are one event, and splitting them
     would double the band count for nothing. */
  function runs(msgs) {
    const out = [];
    let carried = null;
    msgs.forEach((m) => {
      const k = bandClass(m);
      if (m.act) carried = m.act;
      const a = k === 'tool' ? (m.act || carried || 'run') : null;
      const last = out[out.length - 1];
      if (last && last.k === k && last.a === a && last.off === !m.onPath) last.len++;
      else out.push({ k, a, len: 1, off: !m.onPath, start: m.i });
    });
    return out;
  }

  /* The same bands, for a conversation that came out of the index: `cs search --json`
     emits `kind_runs` per row (me9.19) and export.py carries it here untouched, so the
     boundaries the ribbon draws are the ones the terminal would draw beside the same
     conversation rather than a second opinion that happens to agree today.

     Nothing is run-length encoded on this side. The loop walks cs's runs and only
     subdivides a tool run where the act changes — a refinement inside a run, never a
     boundary cs did not send — which is what keeps the act channel above from costing the
     shape its provenance. */
  function wireRuns(conv, msgs) {
    const out = [];
    let carried = null;
    let i = 0;
    conv.shape.forEach(([band, len]) => {
      const k = BAND_CLASS[band] || 'agent';
      for (let j = 0; j < len; j++, i++) {
        const m = msgs[i];
        if (m && m.act) carried = m.act;
        const a = k === 'tool' ? ((m && m.act) || carried || 'run') : null;
        // `j` is what stops two runs cs sent apart from fusing here when the acts inside
        // them agree. Off-path bands cannot arise: `kind_runs` is the head path.
        if (j && out[out.length - 1].a === a) out[out.length - 1].len++;
        else out.push({ k, a, len: 1, off: false, start: i });
      }
    });
    return out;
  }

  function ribbon(conv, withTicks) {
    // Hidden kinds are dimmed, not dropped. Removing them from the axis was worse:
    // the default preset hides tools and reasoning, so every ribbon collapsed to the
    // same teal/white alternation and the list stopped being triageable at all.
    //
    // Undrawn messages *are* off the axis, and that is a different question: nobody hid
    // them with a knob, cs never put them on it. Both channels are then measured in that
    // one space — 563 positions on a 937-message conversation — because a body drawn in
    // drawn-message counts under ticks placed in message counts is the trap me9.25
    // records for `match_seqs`, arriving on the axis by the other door.
    const shown = drawnMsgs(conv);
    const n = shown.length || 1;
    const W = ribbonWidth(n);
    // The track is the length cue: it is the thing that ends early, so a short
    // conversation reads as short rather than as a long one drawn mostly empty.
    let html = `<div class="rb-track" style="width:${W.toFixed(1)}px">`;

    let x = 0;
    // The fixtures in data.js have no index behind them and so carry no shape; that
    // fallback is the only place this prototype still encodes its own runs.
    (conv.shape ? wireRuns(conv, shown) : runs(shown)).forEach((r) => {
      const w = (r.len / n) * W;
      // A steer is one message; give it a floor so it stays a visible separator.
      const px = r.k === 'user' ? Math.max(2, w) : Math.max(0.7, w - 0.4);
      const dim = state.fidelity[r.k] === 'hidden' ? ' dim' : '';
      const act = r.a ? ` a-${r.a}` : '';
      html += `<div class="rb-band ${r.k}${act}${r.off ? ' off' : ''}${dim}" ` +
              `style="left:${x.toFixed(2)}px;width:${px.toFixed(2)}px"></div>`;
      x += w;
    });
    html += '</div>';

    // Over every message, not just the drawn ones, but positioned by how many drawn
    // messages precede it: a mark on something cs does not draw belongs at the position it
    // sits in front of, not nowhere. Rare and real — one pause in a 93-conversation
    // sample, a tool that took four hours to answer — and dropping it silently would be a
    // hole in the graphic rather than a decision about it.
    let at = 0;
    conv.msgs.forEach((m) => {
      const x = (at / n) * W;
      if (m.drawn !== false) at++;
      if (withTicks && m.match) {
        html += `<div class="rb-tick" style="left:${x.toFixed(2)}px"></div>`;
      }
      if (m.isQuestion) html += `<div class="rb-mark q" style="left:${x.toFixed(2)}px"></div>`;
      if (m.err) html += `<div class="rb-mark x" style="left:${x.toFixed(2)}px"></div>`;
      // A pause is a pause wherever it lands.
      if (m.paused) html += `<div class="rb-notch" style="left:${x.toFixed(2)}px"></div>`;
      // A compaction is not a pause. A pause says you went away; this says the earlier
      // half of the conversation stopped being verbatim, so it gets its own mark rather
      // than a heavier notch. Full height, because it cuts the whole thing.
      if (m.compacted) html += `<div class="rb-cut" style="left:${x.toFixed(2)}px"></div>`;
    });

    return html;
  }

  const hasQuery = () => Boolean(state.query.free);

  const root = document.documentElement;

  /* `index.html?dir=paper` renders the whole prototype in one of the visual directions
     in directions.css. A URL rather than a control in the title bar on purpose: the
     title bar is part of what is being judged, and a chooser sitting in it would be
     furniture that ships in none of the four. */
  function applyDirection() {
    root.classList.add(root.classList.contains('light') ? 'theme-light' : 'theme-dark');
    const name = new URLSearchParams(location.search).get('dir');
    if (name && /^[a-z]+$/.test(name)) root.classList.add('dir-' + name);
  }

  function ribbonKey() {
    const k = el('div', 'ribbon-key');
    k.innerHTML =
      KINDS.map(({ k: kk, label }) =>
        `<span style="opacity:${state.fidelity[kk] === 'hidden' ? 0.35 : 1}">` +
        `<i style="background:var(--k-${kk})"></i>${label}</span>`).join('') +
      (hasQuery() ? '<span><i style="background:var(--hit);height:4px"></i>match</span>' : '') +
      '<span><i style="background:var(--sel);border-radius:50%;width:7px;height:7px"></i>asked you</span>' +
      '<span><i style="background:var(--err)"></i>failed</span>' +
      '<span><i style="background:var(--ink-2);width:2px"></i>pause / resume</span>';
    return k;
  }

  /* One row, two views. The projects accordion renders the same component the search
     list does — same ribbon, same grid, same third line — because a conversation inside
     a project is not a different kind of thing, and two implementations would drift.
     `slot` is the only difference: inside a project every row shares one directory, so
     that cell spends itself on the date instead of repeating the path 20 times. */
  function conversationRow(c, slot) {
    const q = hasQuery();
    const b = el('button', 'row');
    b.type = 'button';
    b.setAttribute('role', 'option');
    b.setAttribute('aria-selected', c.id === state.selected ? 'true' : 'false');

    // Two clusters with a gutter between them: who and how big on the left, where and
    // when on the right. `cwd` moved out of the flexible middle to join the date,
    // because both answer "which of my worlds was this" rather than "what is it".
    const ed = c.edits || [0, 0];
    let html =
      '<div class="row-meta">' +
      sourceBadge(c.src) +
      '<span class="model"></span>' +
      `<span class="msgs">${c.n} msgs</span>` +
      // Blank on 194 of 354 sampled conversations — most agent work runs and reads
      // without changing a line — so the column has to be quiet when it is empty.
      `<span class="edits">${ed[0] || ed[1]
        ? `<i class="m">−${ed[0]}</i><i class="p">+${ed[1]}</i>` : ''}</span>` +
      `<span class="ribbon">${ribbon(c, q)}</span>` +
      '<span class="sp"></span>' +
      '<span class="dir"></span>' +
      `<span class="fork">${c.forks > 1 ? '⑂' + c.forks : ''}</span>` +
      `<span class="age">${c.age}</span>` +
      '</div>' +
      '<div class="row-title"></div>';

    // Line 3 only when there is a match to show, so browsing keeps the density.
    if (q && c.bestMatch) {
      const m = c.bestMatch;
      const isTool = m.kind === 'tool_call' || m.kind === 'tool_result';
      html +=
        '<div class="row-snip">' +
        `<span class="sig">${isTool ? '⚙' : m.role === 'user' ? '▸' : '◂'}</span>` +
        (isTool ? `<span class="prov">${m.tool}</span><span class="sig">›</span>` : '') +
        '<span class="frag"></span></div>';
    }

    // Topics last, because they are the only line that is true of the conversation
    // rather than about this query. Plain text, not chips: a `.row` is a <button> and a
    // nested button is invalid, and 354 rows of pills is a lot of furniture. They stay
    // clickable in the rail and the drawer.
    if (c.topics && c.topics.length) {
      html += '<div class="row-topics"></div>';
    }

    b.innerHTML = html;
    if (c.topics && c.topics.length) {
      b.querySelector('.row-topics').textContent = c.topics.join('  ·  ');
    }
    b.querySelector('.model').textContent = c.model || '';
    // Inside a project the run divider above already names the day, so the cell spends
    // itself on the hour instead — which is the one thing the divider cannot say, and
    // makes the rhythm of an afternoon's work visible down the column.
    b.querySelector('.dir').textContent =
      slot === 'clock' ? clockOf(c.endedAt) : elidePath(c.dir, DIR_CHARS);
    b.querySelector('.row-title').textContent = c.title;

    const frag = b.querySelector('.frag');
    if (frag) {
      const m = c.bestMatch;
      const raw = m.frag || m.text;
      // [x] in the fixture marks where the ranker hit; render it as the highlight.
      frag.innerHTML = '“' + raw.replace(/\[([^\]]+)\]/g, '<mark>$1</mark>') + '”';
    }

    b.addEventListener('click', () => selectConversation(c.id));
    return b;
  }

  function viewSearch() {
    const wrap = el('div', 'panes');
    const listCol = el('div', 'col');
    listCol.appendChild(state.group === 'none' ? ribbonKey() : groupKey());

    const list = el('div', 'list');
    list.id = 'list';
    list.setAttribute('role', 'listbox');
    list.setAttribute('aria-label', 'Search results');

    // One list, cut along whichever axis is selected. Projects and Sittings used to be
    // separate views for this, which made a GROUP BY look like a destination and left
    // the grouped surfaces unable to share the filters, the row or the drawer.
    if (state.group === 'none') visible().forEach((c) => list.appendChild(conversationRow(c)));
    else drawGroups(list, groupsFor(visible()));

    listCol.appendChild(list);
    wrap.appendChild(listCol);
    wrap.appendChild(previewCol());

    const shell = el('div', 'searchshell');
    shell.appendChild(wrap);
    if (state.timeline) {
      const d = timelineDrawer();
      if (d) shell.appendChild(d);
    } else {
      const show = el('div', 'tldrawer');
      show.innerHTML = '<div class="tld-h"><span class="sp"></span>' +
        '<button class="tld-btn" data-show="1">show timeline</button></div>';
      show.querySelector('[data-show]').addEventListener('click', () => { state.timeline = true; render(); });
      shell.appendChild(show);
    }
    return shell;
  }

  /* ------------------------------------------------- timeline drawer ------- */
  /* Draws whatever survives every other filter, with query hits raised above the
     baseline — so it answers "when was I working on this" and "when did this query
     land" at once, and scrubbing narrows the same set the list is showing. */

  function timelineDrawer() {
    const all = CONVERSATIONS.filter((c) => c.endedAt);
    if (!all.length) return null;
    const lo0 = Math.min(...all.map((c) => c.endedAt));
    const hi0 = Math.max(...all.map((c) => c.endedAt));
    const pool = visible(true);                 // everything except the range itself

    const wrap = el('div', 'tldrawer');
    const inRange = visible().length;
    const hits = pool.filter((c) => c.bestMatch).length;

    const h = el('div', 'tld-h');
    h.innerHTML =
      `<b>${inRange}</b> in range` +
      (hits ? ` · <b>${hits}</b> with a match` : '') +
      (state.query.range ? ` · ${new Date(state.query.range[0]).toISOString().slice(0, 10)} → ${new Date(state.query.range[1]).toISOString().slice(0, 10)}` : ' · all time') +
      '<span class="sp"></span>' +
      ['30d', '90d', '1y', 'all'].map((z) => `<button class="tld-btn" data-z="${z}">${z}</button>`).join('') +
      '<button class="tld-btn" data-hide="1">hide</button>';
    wrap.appendChild(h);

    const track = el('div', 'tld-track');
    track.innerHTML = '<div class="base"></div>';
    pool.forEach((c) => {
      const m = el('div', 'tld-mark' + (c.bestMatch ? ' hit' : ''));
      const x = ((c.endedAt - lo0) / Math.max(hi0 - lo0, 1)) * 100;
      const tall = Math.max(4, Math.min(18, Math.log10(c.n + 1) * 7));
      m.style.left = x + '%';
      m.style.height = tall + 'px';
      if (c.bestMatch) { m.style.bottom = '50%'; } else { m.style.top = '50%'; }
      m.style.background = c.bestMatch ? 'var(--hit)' : SOURCES[c.src] ? SOURCES[c.src].hue : 'var(--ink-3)';
      m.title = c.title;
      track.appendChild(m);
    });
    if (state.query.range) {
      const sel = el('div', 'tld-sel');
      sel.style.left = (((state.query.range[0] - lo0) / (hi0 - lo0)) * 100) + '%';
      sel.style.width = (((state.query.range[1] - state.query.range[0]) / (hi0 - lo0)) * 100) + '%';
      track.appendChild(sel);
    }
    wrap.appendChild(track);

    const ax = el('div', 'tld-ax');
    ax.innerHTML = `<span>${new Date(lo0).toISOString().slice(0, 10)}</span>` +
                   '<span>hits above the line · sources below</span>' +
                   `<span>${new Date(hi0).toISOString().slice(0, 10)}</span>`;
    wrap.appendChild(ax);

    // scrub
    let anchor = null;
    const at = (e) => {
      const r = track.getBoundingClientRect();
      return lo0 + Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)) * (hi0 - lo0);
    };
    track.addEventListener('pointerdown', (e) => { anchor = at(e); track.setPointerCapture(e.pointerId); });
    track.addEventListener('pointerup', (e) => {
      if (anchor == null) return;
      const b2 = at(e);
      state.query.range = Math.abs(b2 - anchor) < (hi0 - lo0) * 0.01 ? null : [Math.min(anchor, b2), Math.max(anchor, b2)];
      anchor = null;
      render();
    });

    h.querySelectorAll('[data-z]').forEach((b2) => b2.addEventListener('click', () => {
      const d = { '30d': 30, '90d': 90, '1y': 365 }[b2.dataset.z];
      state.query.range = d ? [hi0 - d * 86400000, hi0] : null;
      render();
    }));
    h.querySelector('[data-hide]').addEventListener('click', () => { state.timeline = false; render(); });
    return wrap;
  }

  /* ------------------------------------------------------------- preview --- */

  function block(m, opts) {
    const o = opts || {};
    const d = el('div', 'blk ' + bandClass(m) + (m.err ? ' err' : '') +
                (m.onPath ? '' : ' off') + (m.isQuestion ? ' q' : ''));
    let sig = '';
    let text = '';
    let html = false;

    const level = o.level || levelOf(m);

    if (m.kind === 'prose') {
      sig = m.role === 'user' ? '▸' : m.isQuestion ? '?' : '◂';
      const full = level === 'expanded' && m.textLong ? m.textLong : m.text;
      text = level === 'collapsed'
        ? m.text.slice(0, 78) + (m.text.length > 78 ? '…' : '')
        : full;
      if (m.match) {
        text = text.replace(/retention|window|timezone/gi, (x) => `<mark>${x}</mark>`);
        html = true;
      }
    } else if (m.kind === 'reasoning') {
      sig = '⋯';
      text = level === 'expanded'
        ? `${m.text} · ${Math.round(m.len / 52)} lines`
        : `reasoning · ${Math.round(m.len / 52)} lines`;
    } else if (m.kind === 'tool_call') {
      // The glyph says what the call was for. In the preview there is room for the
      // distinction to be legible, which is where the ribbon's sub-shades cannot be.
      sig = ACT_GLYPH[m.act] || '⚙';
      d.classList.add('a-' + (m.act || 'run'));
      // The argument, not the tool name. `Bash` told you nothing; `ls -la && echo …`
      // is the line. Collapsed truncates it, expanded gives the whole thing — which is
      // now a reason to expand a tool line rather than a no-op.
      const name = m.call || m.tool;
      const arg = m.arg || '';
      const shown = level === 'expanded' ? arg : arg.slice(0, 84) + (arg.length > 84 ? '…' : '');
      html = true;
      text = `<b class="cmd">${esc(name)}</b>${arg ? ' ' + esc(shown) : ''}` +
             (m.diff ? diffBadge(m.diff) : '') +
             (level === 'expanded' ? `  <span class="kb">${(m.len / 1024).toFixed(1)} KB</span>` : '');
    } else {
      // tool_result omitted: the call implies it. A failure is not — a tool breaking
      // is recognition information. TUI-DESIGN §8.
      // A successful result is omitted unless tools are fully expanded — the call
      // already implies it. A failure always survives. TUI-DESIGN §8.
      // Never disappear under a click. The kind-level default may omit a successful
      // result — the call implies it — but a message the reader has touched directly
      // always renders, or clicking it deletes it with no way back.
      if (!m.err && level !== 'expanded' && !state.overrides.has(ovrKey(m))) return null;
      if (m.err) { sig = '✕'; text = 'Error: no such column: message.text_hash'; }
      else { sig = '↳'; text = `${(m.len / 1024).toFixed(1)} KB`; }
    }

    if (state.overrides.has(ovrKey(m))) d.classList.add('ovr');
    d.innerHTML = `<div class="gut"></div><span class="sig">${sig}</span><span class="txt"></span>`;
    const t = d.querySelector('.txt');
    if (html) t.innerHTML = text; else t.textContent = text;

    t.addEventListener('click', () => {
      const key = ovrKey(m);
      const now = state.overrides.get(key) || level;
      if (now === 'expanded') state.overrides.set(key, 'collapsed');
      else state.overrides.set(key, 'expanded');
      render();
    });
    return d;
  }

  function breakRule(label) {
    const b = el('div', 'brk');
    b.textContent = label;
    return b;
  }

  /* Louder than a pause rule, because it means something worse: at seq 924 of 1,323 you
     would never find this by scrolling, and everything above it is a summary. */
  function cutRule() {
    const b = el('div', 'brk cut');
    b.innerHTML = '<span>context compacted &middot; everything above is summarised ' +
      'from here on</span>';
    return b;
  }

  const esc = (t) => String(t).replace(/[&<>]/g, (ch) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[ch]));

  /* Edits carry their own diff: an Edit is a replacement, so the line counts of
     old_string and new_string are the same +/- a diff would show without needing the
     file. apply_patch counts the patch body directly. 2,278 calls in the sample. */
  const diffBadge = ([minus, plus]) =>
    '  <span class="dif">' +
    (minus ? `<i class="m">−${minus}</i>` : '') +
    (plus ? `<i class="p">+${plus}</i>` : '') + '</span>';

  function hours(ms) {
    const h = ms / 3600000;
    return h >= 24 ? `${Math.round(h / 24)}d later` : `${Math.round(h)}h later`;
  }

  const ACT_ORDER = ['run', 'change', 'look', 'steer'];

  /* One line for the shape of the work, one for where it landed. Both are free: the act
     comes from the tool name, the paths were already mined for the project rollup and
     were being thrown away at conversation scale. */
  function workSummary(c) {
    const calls = c.msgs.filter((m) => m.act);
    const side = c.sidechain || 0;
    if (!calls.length && !c.files.length && side < 0.05) return '';

    const n = new Map();
    calls.forEach((m) => n.set(m.act, (n.get(m.act) || 0) + 1));
    // A share that rounds to 0% is noise on the line; the count says the thing happened
    // without claiming it was a proportion of the work.
    const acts = ACT_ORDER.filter((a) => n.get(a)).map((a) => {
      const pct = Math.round((n.get(a) / calls.length) * 100);
      return `<span class="act ${a}"><i>${ACT_GLYPH[a]}</i>${a}` +
             `<b>${pct >= 1 ? pct + '%' : '×' + n.get(a)}</b></span>`;
    });

    // Subagents share the `did` row rather than taking one of their own: they appear on
    // 57 conversations in the whole corpus, and a label that is blank on 98% of rows is
    // a row nobody should be paying vertical space for.
    const sub = side >= 0.05
      ? `<span class="act sub"><i>⑃</i>subagents<b>${Math.round(side * 100)}%</b></span>`
      : '';

    let html = '<div class="pv-work">';
    if (calls.length || sub) {
      html += '<div class="pv-acts"><span class="gname">did</span>' +
        (calls.length ? `<span class="n">${calls.length} call${calls.length > 1 ? 's' : ''}</span>` : '') +
        acts.join('') + sub + '</div>';
    }
    if (c.files.length) {
      // Three, so the row cannot wrap. The count carries the rest.
      html += '<div class="pv-acts"><span class="gname">touched</span>' +
        c.files.slice(0, 3).map(() =>
          '<span class="tchip sm ghost"><span class="nm"></span></span>').join('') +
        (c.files.length > 3 ? `<span class="n">+${c.files.length - 3}</span>` : '') +
        '</div>';
    }
    return html + '</div>';
  }

  function previewCol() {
    const c = CONVERSATIONS.find((x) => x.id === state.selected) || CONVERSATIONS[0];
    const col = el('div', 'col preview');
    const preset = activePreset();

    const h = el('div', 'pv-head');
    h.innerHTML =
      '<div class="pv-title"></div>' +
      '<div class="pv-meta"></div>' +
      // Folded to one line. Sixteen chips took four rows and pushed the controls halfway
      // down the drawer, which is a lot of space for a facet you can also reach from the
      // rail. The count on the fold says what is behind it.
      (() => {
        const t = ((REAL && REAL.topics) || []).filter((x) => x.members.includes(c.id));
        if (!t.length) return '<div class="pv-tags"><span class="tchip sm ghost">no topic</span></div>';
        const open = state.pvTopics;
        const shown = open ? t : t.slice(0, 3);
        return '<div class="pv-tags">' + shown.map((x) =>
          `<span class="tchip sm${hasTopic(x.name) ? ' on' : ''}" data-topic="${x.name.replace(/"/g, '')}">${x.name}</span>`
        ).join('') +
          (t.length > 3
            ? `<button type="button" class="tchip sm more" data-tagfold="1">${open ? 'fewer' : `+${t.length - 3}`}</button>`
            : '') +
          '</div>';
      })() +
      // What the conversation *did*, as opposed to what it was made of. The ribbon can
      // say "this is mostly tool traffic"; only this can say whether that traffic was
      // reading, editing or executing, and which files it landed on.
      workSummary(c) +
      '<div class="zoom" id="zoom" role="group" aria-label="Preset">' +
      Object.keys(PRESETS)
        .map((z) => `<button type="button" data-zoom="${z}" aria-pressed="${preset === z}">${z}</button>`)
        .join('') +
      (preset ? '' : '<button type="button" aria-pressed="true" disabled>custom</button>') +
      '</div>' +
      // Label, state and dot inside one box. The 2x2 grid this replaces put `you`'s
      // control nearer to `agent`'s label than `agent`'s own control was, so proximity
      // pointed at the wrong thing on every read.
      '<div class="fid" id="fid" role="group" aria-label="Detail per message kind">' +
      KINDS.map(({ k, label }) => {
        const lv = state.fidelity[k];
        return (
          `<div class="fid-chip${lv === 'hidden' ? '' : ' on'}">` +
          `<button type="button" class="dot" data-vis="${k}" aria-pressed="${lv !== 'hidden'}" ` +
          `title="${label}: ${lv === 'hidden' ? 'show' : 'hide'}" ` +
          `aria-label="${label}: ${lv === 'hidden' ? 'show' : 'hide'}" ` +
          `style="--k:var(--k-${k})"></button>` +
          `<button type="button" class="lv" data-kind="${k}" ` +
          `title="${label}: ${LEVEL_WORD[lv]} — click to cycle, shift-click to go back" ` +
          `aria-label="${label}: ${LEVEL_WORD[lv]}">` +
          `<span class="nm">${label}</span><span class="st">${LEVEL_WORD[lv]}</span>` +
          '</button></div>'
        );
      }).join('') +
      '</div>' +
      (state.overrides.size
        ? '<div class="fid-all" id="fid-all"><button type="button" data-all="clear">' +
          `clear ${state.overrides.size} per-message override${state.overrides.size > 1 ? 's' : ''}` +
          '</button></div>'
        : '');
    h.querySelector('.pv-title').textContent = c.title;

    // The drawer used to print the cwd as text, so the one navigable relation on the
    // screen was half wired: its topic chips filtered, its project did not. You could
    // get from a project to a conversation and never back.
    const meta = h.querySelector('.pv-meta');
    meta.textContent = `${c.src} · `;
    if (c.project) {
      const link = el('button', 'pv-proj');
      link.type = 'button';
      link.dataset.dir = c.project;
      link.textContent = (PROJECT_BY_PATH.get(c.project) || { name: c.project }).name;
      meta.appendChild(link);
    } else {
      meta.appendChild(document.createTextNode(c.dir));
    }
    meta.appendChild(document.createTextNode(
      ` · ${c.n} msgs · ${c.segments.length} segments · ${c.span}`));
    // Model never changes inside a conversation — 0 of 3,057 carry more than one — so it
    // is a property of the whole thing rather than something to draw along the axis.
    if (c.model) meta.appendChild(document.createTextNode(` · ${c.model}`));

    // Mined paths go in through the DOM, not the template: they are the one part of this
    // markup that was not authored here.
    h.querySelectorAll('.pv-work .tchip .nm').forEach((n, i) => {
      const f = c.files[i];
      if (!f) return;
      n.textContent = elidePath(f, 30);
      n.parentElement.title = f;
    });

    col.appendChild(h);

    const pv = el('div', 'pv');
    const body = el('div', 'pv-body');
    body.id = 'pv-body';

    const show = isShown;
    // Segments mode is what "tools hidden, agent collapsed" means in practice: the
    // run is summarised rather than listed.
    const segmentMode = state.fidelity.tool === 'hidden';

    if (segmentMode) {
      c.segments.forEach((sg, idx) => {
        const paused = sg.items.find((m) => m.paused);
        if (paused) body.appendChild(breakRule(hours(paused.gapShown || FOUR_HOURS * 1.5)));
        if (sg.items.some((m) => m.compacted) || (sg.steer && sg.steer.compacted)) {
          body.appendChild(cutRule());
        }
        // A steer with nothing after it is not a segment worth a summary line.
        if (!sg.items.length && !sg.steer) return;

        const wrapSeg = el('div', 'seg');
        if (sg.steer && show(sg.steer)) wrapSeg.appendChild(block(sg.steer));
        if (!sg.items.length) { body.appendChild(wrapSeg); return; }

        const open = state.openSegments.has(c.id + ':' + idx);
        const bits = [];
        if (sg.calls) bits.push(`${sg.calls} calls`);
        if (sg.fails) bits.push(`<b>${sg.fails} failed</b>`);
        if (sg.questions) bits.push(`<i>asked you ${sg.questions}×</i>`);
        if (!bits.length) bits.push(`${sg.items.length} messages`);

        const sum = el('button', 'seg-sum');
        sum.type = 'button';
        sum.innerHTML =
          `<div class="gut"></div><span class="caret">${open ? '▾' : '▸'}</span>` +
          `<span>→ ${bits.join(' · ')}</span>` +
          `<span class="caret">${sg.matches ? '●' : ''}</span>`;
        sum.addEventListener('click', () => {
          const key = c.id + ':' + idx;
          if (state.openSegments.has(key)) state.openSegments.delete(key);
          else state.openSegments.add(key);
          render();
        });
        wrapSeg.appendChild(sum);

        if (open) {
          // Opening a segment reveals what the kind controls hid, but respects the
          // level they chose — hidden reads as collapsed here rather than expanded.
          sg.items.forEach((m) => {
            if (!m.onPath && !state.showOffPath) return;
            const lv = levelOf(m);
            const b = block(m, { level: lv === 'hidden' ? 'collapsed' : lv });
            if (b) wrapSeg.appendChild(b);
          });
        }
        body.appendChild(wrapSeg);
      });
    } else {
      let segIdx = 0;
      c.msgs.forEach((m) => {
        const sg = c.segments[segIdx];
        if (sg && m.i === sg.start && sg.gapMs > FOUR_HOURS) {
          body.appendChild(breakRule(hours(sg.gapMs)));
        }
        if (sg && m.i >= sg.end) segIdx++;
        if (!show(m)) return;
        const b = block(m);
        if (b) body.appendChild(b);
      });
    }

    /* minimap — same object, highest resolution */
    const mm = el('div', 'mm');
    mm.id = 'mm';
    mm.tabIndex = 0;
    mm.setAttribute('role', 'slider');
    mm.setAttribute('aria-label', 'Conversation minimap');
    mm.setAttribute('aria-valuemin', '0');
    mm.setAttribute('aria-valuemax', '100');
    mm.setAttribute('aria-valuenow', '0');

    // Same rule as the ribbon: dim rather than drop, so the scrollbar keeps
    // describing the whole conversation.
    const msgs = c.msgs.filter((m) => m.onPath || state.showOffPath);
    const w = msgs.map((m) => Math.log10(m.len));
    const total = w.reduce((a, b) => a + b, 0);
    let acc = 0;
    msgs.forEach((m, i) => {
      const top = (acc / total) * 100;
      const hh = (w[i] / total) * 100;
      acc += w[i];
      const band = el('div', 'mm-band ' + bandClass(m) + (m.onPath ? '' : ' off') +
                    (levelOf(m) === 'hidden' ? ' dim' : ''));
      band.style.top = top + '%';
      band.style.height = Math.max(0.35, hh - 0.12) + '%';
      band.style.width = bandClass(m) === 'user' ? '18px' : '28px';
      mm.appendChild(band);
      if (m.match && hasQuery()) {
        const tick = el('div', 'mm-hit');
        tick.style.top = `calc(${top}% + 1px)`;
        mm.appendChild(tick);
      }
      if (m.paused) {
        const nt = el('div', 'mm-notch');
        nt.style.top = top + '%';
        mm.appendChild(nt);
      }
      // The minimap is how you cross a 1,300-message conversation, so the one boundary
      // that changes what the agent knew belongs on it too.
      if (m.compacted) {
        const ct = el('div', 'mm-cut');
        ct.style.top = top + '%';
        mm.appendChild(ct);
      }
    });
    const vbox = el('div', 'mm-view');
    vbox.id = 'mm-view';
    mm.appendChild(vbox);

    pv.appendChild(body);
    pv.appendChild(mm);
    col.appendChild(pv);
    return col;
  }

  /* ================================================== grouping ============ */
  /* Projects and Sittings were both GROUP BY over the same set wearing the costume of
     destinations: same rows, same drawer, nothing in one and not the others. Making
     grouping a dimension collapses them into settings and yields two more axes free —
     topic, which was filterable but never browsable, and source, which matters because
     archetype is ~87% predicted by it.

     A run and a sitting were also the same primitive at two parameterisations: both
     cluster `ended_at`. One was a divider, one was a third of the top-level navigation.
     Now there is one renderer and one word. */

  const RUN_GAP = 12 * 3600000;  // measured: 3 days left whole projects undivided (§1)
  const ROW_CAP = 20;
  const SMALL = 3;               // 38 of 67 projects hold this many or fewer

  function runsOf(items) {
    const out = [];
    items.forEach((c) => {
      const last = out[out.length - 1];
      if (last && last.end - (c.endedAt || 0) < RUN_GAP) {
        last.items.push(c);
        last.start = c.endedAt || last.start;
      } else {
        out.push({ items: [c], start: c.endedAt || 0, end: c.endedAt || 0 });
      }
    });
    return out;
  }

  function sparkline(items, bars) {
    const ts = items.map((c) => c.endedAt).filter(Boolean);
    // Below three there is no shape, only empty buckets and a spike — which reads as a
    // dotted rule and made the small-project fold look broken.
    if (ts.length < 3) return '<span class="spark"></span>';
    const lo = Math.min(...ts), hi = Math.max(...ts);
    const buckets = new Array(bars).fill(0);
    ts.forEach((t) => {
      buckets[hi === lo ? bars - 1 : Math.round(((t - lo) / (hi - lo)) * (bars - 1))]++;
    });
    const peak = Math.max(1, ...buckets);
    return '<span class="spark">' + buckets.map((n) =>
      `<i style="height:${n ? Math.max(3, Math.round((n / peak) * 16)) : 1.5}px"` +
      `${n ? '' : ' class="z"'}></i>`).join('') + '</span>';
  }

  const clockOf = (ts) => {
    if (!ts) return '';
    const d = new Date(ts);
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  };

  function statsOf(items) {
    let err = 0, asked = 0, pauses = 0, msgs = 0;
    items.forEach((c) => {
      msgs += c.n;
      c.msgs.forEach((m) => {
        if (m.err) err++;
        if (m.isQuestion) asked++;
        if (m.paused) pauses++;
      });
    });
    return { err, asked, pauses, msgs };
  }

  /* ---------------------------------------------------- the four axes ----- */

  function groupsFor(pool) {
    if (state.group === 'project') return projectGroups(pool);
    if (state.group === 'run') return runGroups(pool);
    if (state.group === 'topic') return topicGroups(pool);
    return sourceGroups(pool);
  }

  function projectGroups(pool) {
    const by = new Map();
    pool.forEach((c) => {
      if (!c.project) return;
      if (!by.has(c.project)) by.set(c.project, []);
      by.get(c.project).push(c);
    });
    by.forEach((items) => items.sort((a, b) => (b.endedAt || 0) - (a.endedAt || 0)));

    // Most recently touched first. Sorting by size put a finished October project above
    // what you did yesterday.
    return [...by.entries()]
      .sort((x, y) => (y[1][0].endedAt || 0) - (x[1][0].endedAt || 0))
      .map(([path, items]) => {
        const p = PROJECT_BY_PATH.get(path);
        const total = p ? p.n : items.length;
        return {
          key: path, items, total, small: total <= SMALL, runs: true,
          label: p ? p.name : elidePath(path, 34),
          first: p && p.first ? p.first : items[items.length - 1].endedAt,
          last: p && p.last ? p.last : items[0].endedAt,
          band: () => projectBand(p, items),
          narrow: () => { toggleDir(path); state.group = 'none'; keepSelection(); },
        };
      });
  }

  /* A run across everything, not within a project: the old Sittings view, minus the
     requirement that it cross two tools — which was a display filter masquerading as a
     definition and threw most of them away. Cross-tool runs are marked instead. */
  function runGroups(pool) {
    const sorted = pool.slice().sort((a, b) => (b.endedAt || 0) - (a.endedAt || 0));
    return runsOf(sorted).map((r) => {
      const tools = [...new Set(r.items.map((c) => c.src))].sort();
      const projects = [...new Set(r.items.map((c) => c.project).filter(Boolean))];
      const from = dayLabel(r.start, true), to = dayLabel(r.end, true);
      return {
        key: String(r.end), items: r.items, total: r.items.length,
        label: from === to ? to : `${from} – ${to}`,
        first: r.start, last: r.end,
        // The label is already the date; repeating it in the date column says nothing.
        // The hours do, and they are what distinguishes a morning from an all-nighter.
        when: `${clockOf(r.start)} – ${clockOf(r.end)}`,
        crossTool: tools.length > 1,
        band: () => runBand(r, tools, projects),
        narrow: null,   // no grammar for an arbitrary window - see chat-search-4ar.10
      };
    });
  }

  function topicGroups(pool) {
    const topics = (REAL && REAL.topics) || [];
    const groups = topics.map((t) => {
      const items = pool.filter((c) => t.members.includes(c.id))
                        .sort((a, b) => (b.endedAt || 0) - (a.endedAt || 0));
      return {
        key: t.name, items, total: items.length, label: t.name, facet: t.facet,
        first: items.length ? items[items.length - 1].endedAt : 0,
        last: items.length ? items[0].endedAt : 0,
        band: () => topicBand(t, items),
        narrow: () => { toggleTopic(t.name); state.group = 'none'; keepSelection(); },
      };
    }).filter((g) => g.items.length).sort((x, y) => y.total - x.total);

    // 24% of the corpus matches no topic. A grouping that silently drops a quarter of
    // the rows is lying about the set, so the residue gets a group of its own.
    const tagged = new Set(topics.flatMap((t) => t.members));
    const rest = pool.filter((c) => !tagged.has(c.id))
                     .sort((a, b) => (b.endedAt || 0) - (a.endedAt || 0));
    if (rest.length) {
      groups.push({
        key: ' untagged', items: rest, total: rest.length, untagged: true,
        label: 'no topic matched',
        first: rest[rest.length - 1].endedAt, last: rest[0].endedAt,
        band: () => untaggedBand(rest, pool.length),
        narrow: null,
      });
    }
    return groups;
  }

  function sourceGroups(pool) {
    const by = new Map();
    pool.forEach((c) => {
      if (!by.has(c.src)) by.set(c.src, []);
      by.get(c.src).push(c);
    });
    by.forEach((items) => items.sort((a, b) => (b.endedAt || 0) - (a.endedAt || 0)));
    return [...by.entries()].sort((x, y) => y[1].length - x[1].length).map(([src, items]) => ({
      key: src, items, total: items.length, label: src, src,
      first: items[items.length - 1].endedAt, last: items[0].endedAt,
      band: () => sourceBand(src, items),
      narrow: () => { toggleAgent(src); state.group = 'none'; },
    }));
  }

  /* ---------------------------------------------------------- bands ------- */

  const chipRow = (label, html) =>
    `<div class="pj-sec"><span class="gname">${label}</span>${html}</div>`;

  const numRow = (bits) =>
    `<div class="pj-sec pj-nums">${bits.filter(Boolean).join('')}</div>`;

  function statBits(items) {
    const st = statsOf(items);
    return [
      `<span>${st.msgs.toLocaleString()} messages</span>`,
      st.asked ? `<span class="q">${st.asked} question${st.asked > 1 ? 's' : ''} back to you</span>` : '',
      st.err ? `<span class="e">${st.err} failed call${st.err > 1 ? 's' : ''}</span>` : '',
      st.pauses ? `<span>${st.pauses} pause${st.pauses > 1 ? 's' : ''} over 4h</span>` : '',
    ];
  }

  function projectBand(p, items) {
    const band = el('div', 'pj-band');
    const path = el('div', 'pj-path');
    path.textContent = p ? p.path : items[0].dir;
    band.appendChild(path);

    const inner = el('div');
    inner.innerHTML =
      (p && p.topics.length ? chipRow('topics', p.topics.slice(0, 6).map((x) =>
        `<button type="button" class="tchip sm${x.facet === 'mode' ? ' ghost' : ''}" ` +
        `data-topic="${x.name}" title="${x.facet}"><span class="nm">${x.name}</span>` +
        `<span class="c">${x.n}</span></button>`).join('')) : '') +
      (p && p.files.length ? chipRow('files', p.files.slice(0, 5).map((x) =>
        '<span class="tchip sm ghost"><span class="nm"></span>' +
        `<span class="c">${x.n}</span></span>`).join('')) : '') +
      numRow(statBits(items).concat(
        p && p.dirs > 1 ? `<span>${p.dirs} directories, worktrees folded in</span>` : ''));
    // Mined paths go in through the DOM, for the same reason as in runBand.
    inner.querySelectorAll('.tchip.ghost').forEach((chip, i) => {
      const f = p.files[i];
      if (!f) return;
      chip.title = f.path;
      chip.querySelector('.nm').textContent = elidePath(f.path, 34);
    });
    band.appendChild(inner);
    return band;
  }

  function runBand(r, tools, projects) {
    const band = el('div', 'pj-band');
    const mins = Math.round((r.end - r.start) / 60000);
    const span = mins < 60 ? `${mins}m` : `${Math.floor(mins / 60)}h ${mins % 60}m`;
    band.innerHTML =
      `<div class="pj-path">${clockOf(r.start)} to ${clockOf(r.end)} · ${span}</div>` +
      (projects.length ? chipRow('projects', projects.slice(0, 5).map(() =>
        '<button type="button" class="tchip sm"><span class="nm"></span></button>').join('')) : '') +
      chipRow('tools', tools.map((t) => sourceBadge(t)).join('')) +
      numRow(statBits(r.items));
    // Paths and names go in through the DOM, not through the template. They are mined
    // out of tool-call text and cwd columns, so they are the one part of this markup
    // that is not authored here.
    band.querySelectorAll('.tchip .nm').forEach((n, i) => {
      const p = projects[i];
      if (p === undefined) return;
      n.textContent = (PROJECT_BY_PATH.get(p) || { name: p }).name;
      n.parentElement.dataset.dir = p;
    });
    return band;
  }

  function topicBand(t, items) {
    const band = el('div', 'pj-band');
    // A seeded topic explains itself by the seeds that fired, ranked by members reached.
    // Mining distinctive terms failed twice; these groups have a stated reason instead.
    band.innerHTML =
      `<div class="pj-path">${t.facet} · ${t.group} · matched by keyword, not by model</div>` +
      chipRow('seeds', (t.terms || []).map((x) =>
        `<span class="tchip sm ghost"><span class="nm">${x}</span></span>`).join('')) +
      numRow(statBits(items));
    return band;
  }

  function sourceBand(src, items) {
    const band = el('div', 'pj-band');
    const prose = items.reduce((a, c) =>
      a + c.msgs.filter((m) => m.kind === 'prose').length, 0);
    const msgs = items.reduce((a, c) => a + c.n, 0) || 1;
    band.innerHTML =
      `<div class="pj-path">${(msgs / items.length).toFixed(1)} messages per conversation · ` +
      `${Math.round((prose / msgs) * 100)}% prose</div>` +
      numRow(statBits(items));
    return band;
  }

  function untaggedBand(rest, poolSize) {
    const band = el('div', 'pj-band');
    band.innerHTML =
      `<div class="pj-path">${Math.round((rest.length / poolSize) * 100)}% of this ` +
      'selection matches none of the 26 seeds</div>' +
      '<div class="pj-sec"><span class="note-in">A seeded taxonomy is allowed no ' +
      'opinion, which is the point of it over clustering: k-means assigns everything ' +
      'and always produces a leftovers bucket. This group is the honest residue, not a ' +
      'failure.</span></div>' + numRow(statBits(rest));
    return band;
  }

  /* ---------------------------------------------------- the renderer ------ */

  function groupKey() {
    const k = el('div', 'ribbon-key');
    const g = GROUPINGS.find((x) => x.k === state.group);
    const pool = visible();
    const groups = groupsFor(pool);
    const covered = new Set(groups.flatMap((x) => x.items.map((c) => c.id))).size;
    k.innerHTML =
      `<span><b style="color:var(--ink-2)">${groups.length}</b> groups</span>` +
      `<span>${covered} of ${pool.length} rows land in one</span>` +
      `<span style="opacity:.7">${g.note}</span>`;
    return k;
  }

  function groupSection(g, open) {
    const sec = el('div', 'pj' + (open ? ' open' : '') + (g.small ? ' small' : ''));
    const id = state.group + ':' + g.key;

    const head = el('button', 'pj-head');
    head.type = 'button';
    head.setAttribute('aria-expanded', open ? 'true' : 'false');
    const from = dayLabel(g.first, true), to = dayLabel(g.last, true);
    const srcs = [...new Set(g.items.map((c) => c.src))].sort();

    head.innerHTML =
      '<span class="tw">&#9656;</span><span class="nm"></span>' +
      (g.facet === 'mode' ? '<span class="tag">mode</span>' : '') +
      (g.crossTool ? '<span class="tag">cross-tool</span>' : '') +
      (g.untagged ? '<span class="tag warn">residue</span>' : '') +
      `<span class="srcs">${g.src ? '' : srcs.map((s) => sourceBadge(s, 'tiny')).join('')}</span>` +
      `<span class="cnt"><b>${g.total}</b>` +
      `${g.total !== g.items.length ? ` <i>· ${g.items.length} here</i>` : ''}</span>` +
      `<span class="when">${g.when || (from === to ? to : `${from} – ${to}`)}</span>` +
      sparkline(g.items, 12);
    head.querySelector('.nm').textContent = g.label;
    head.addEventListener('click', () => {
      if (open) state.expanded.delete(id); else state.expanded.add(id);
      render();
    });
    sec.appendChild(head);
    if (!open) return sec;

    const body = el('div', 'pj-body');
    if (g.band) body.appendChild(g.band());

    // Cut first, then divide. Dividing first let a divider announce "26 conversations"
    // above a run the cap had already truncated to 20.
    const capped = state.uncapped.has(id) ? g.items : g.items.slice(0, ROW_CAP);
    if (g.runs) {
      runsOf(capped).forEach((run) => {
        const a = dayLabel(run.end, true), b = dayLabel(run.start, true);
        const div = el('div', 'pj-run');
        div.innerHTML = `<span class="d">${a === b ? a : `${a} - ${b}`}</span>` +
          `<span class="n">${run.items.length} conversation${run.items.length > 1 ? 's' : ''}</span>`;
        body.appendChild(div);
        run.items.forEach((c) => body.appendChild(conversationRow(c, 'clock')));
      });
    } else {
      capped.forEach((c) => body.appendChild(conversationRow(c)));
    }

    const rest = g.total - capped.length;
    if (rest > 0) {
      const more = el('button', 'pj-more');
      more.type = 'button';
      // Narrowing to the group beats growing a nested scroller: one list implementation,
      // and where a facet exists the handoff is a real query you could have typed.
      more.innerHTML = `<span>${rest} more</span><span class="go">` +
        `${g.narrow ? 'narrow to this group' : 'show all'}</span>`;
      more.addEventListener('click', () => {
        if (g.narrow) g.narrow(); else state.uncapped.add(id);
        render();
      });
      body.appendChild(more);
    }
    sec.appendChild(body);
    return sec;
  }

  function drawGroups(list, groups) {
    if (!groups.length) {
      const e = el('div', 'empty');
      e.style.margin = '14px';
      e.innerHTML = '<b>Nothing to group</b>No conversation in this selection carries ' +
        'the column this axis groups by.';
      list.appendChild(e);
      return;
    }

    const big = groups.filter((g) => !g.small);
    const small = groups.filter((g) => g.small);
    big.forEach((g) => list.appendChild(
      groupSection(g, state.expanded.has(state.group + ':' + g.key))));

    // Half the project list is one- to three-conversation projects: all of the rows and
    // none of the work. They fold, but they are counted in the open, because a hidden
    // row and a row that does not exist have to look different.
    if (small.length) {
      const n = small.reduce((a, g) => a + g.items.length, 0);
      const t = el('button', 'pj-tail');
      t.type = 'button';
      t.setAttribute('aria-expanded', state.smallShown ? 'true' : 'false');
      t.innerHTML = '<span class="tw">&#9656;</span>' +
        `<span>${small.length} with ${SMALL} conversations or fewer</span>` +
        `<span class="n">${n}</span>`;
      t.addEventListener('click', () => { state.smallShown = !state.smallShown; render(); });
      list.appendChild(t);
      if (state.smallShown) {
        small.forEach((g) => list.appendChild(
          groupSection(g, state.expanded.has(state.group + ':' + g.key))));
      }
    }
  }

  /* ================================================== view: library ======= */
  /* Everything else in the app is derived from the index and can be thrown away with it.
     This is the half that cannot: what you pinned, named, merged or dismissed. It had no
     home, which is why Collections kept failing to find a tab — it was the only authored
     thing, competing with derived views for space.

     Most of it is empty, and says so. Nothing has been authored yet; that is the true
     state of the app, and these are the first empty states in the prototype. */

  function libSection(title, note, body) {
    const s = el('section', 'lib-sec');
    const h = el('div', 'lib-h');
    h.innerHTML = '<b></b><span></span>';
    h.querySelector('b').textContent = title;
    h.querySelector('span').textContent = note;
    s.appendChild(h);
    s.appendChild(body);
    return s;
  }

  function libEmpty(what, why) {
    const e = el('div', 'empty');
    e.innerHTML = '<b></b><span></span>';
    e.querySelector('b').textContent = what;
    e.querySelector('span').textContent = why;
    return e;
  }

  function viewLibrary() {
    const panes = el('div', 'panes');
    const col = el('div', 'col');
    const wrap = el('div', 'library');

    // A collection is a rule, not a membership list. Storing the rule is what survives a
    // reindex; storing the members freezes an answer the corpus has already moved past.
    const saved = el('div');
    if (!collections.length) {
      saved.appendChild(libEmpty('No collections yet',
        'A collection is stored as matches(query) + pinned - excluded, so it keeps ' +
        'answering after a reindex instead of freezing one answer. Accept a proposal ' +
        'below to make the first one.'));
    } else {
      collections.forEach((k) => {
        const r = resolve(k);
        const b = el('div', 'lib-row');
        b.innerHTML = '<span class="t"></span><span class="rule"></span>' +
                      `<span class="n">${r.length}</span>`;
        b.querySelector('.t').textContent = k.name;
        b.querySelector('.rule').textContent =
          `${k.query} + ${k.pinned.length} pinned - ${k.excluded.length} excluded`;
        saved.appendChild(b);
      });
    }
    wrap.appendChild(libSection('Collections', 'a rule, evaluated - not a list', saved));

    // Proposals. A machine that must have an opinion about every conversation produces a
    // leftovers bucket; these are seeded, so they are allowed to say nothing.
    const props = el('div');
    const live = proposals.filter((p) => !state.dismissed.has(p.id) && !state.accepted.has(p.id));
    live.slice(0, 8).forEach((p) => {
      const row = el('div', 'lib-row prop');
      row.innerHTML = '<span class="t"></span>' +
        `<span class="rule">${(p.terms || []).slice(0, 4).join(' · ')}</span>` +
        `<span class="n">${p.members ? p.members.length : 0}</span>` +
        '<span class="acts"><button type="button" data-act="accept">accept</button>' +
        '<button type="button" data-act="dismiss">dismiss</button></span>';
      row.querySelector('.t').textContent = p.name;
      row.querySelector('[data-act="accept"]').addEventListener('click', () => {
        state.accepted.add(p.id);
        collections.push({
          id: p.id, name: p.name,
          query: (p.seed || []).slice(0, 6).join(' OR '), pinned: [], excluded: [],
        });
        render();
      });
      row.querySelector('[data-act="dismiss"]').addEventListener('click', () => {
        state.dismissed.add(p.id);
        render();
      });
      props.appendChild(row);
    });
    if (!live.length) {
      props.appendChild(libEmpty('Nothing proposed',
        'Every proposal has been accepted or dismissed.'));
    }
    wrap.appendChild(libSection('Proposed',
      `${live.length} seeded topics · being wrong costs one dismissal`, props));

    // The one fact about projects that is genuinely not derivable: a directory that
    // moved. Two rows at opposite ends of a list sorted by count, one project.
    const merges = el('div');
    const pairs = mergeCandidates();
    if (!pairs.length) {
      merges.appendChild(libEmpty('No merges suggested', 'No two projects share a name.'));
    }
    pairs.forEach(([a, b]) => {
      const row = el('div', 'lib-row prop');
      row.innerHTML = '<span class="t"></span><span class="rule"></span>' +
        `<span class="n">${a.n + b.n}</span>` +
        '<span class="acts"><button type="button" data-act="merge">merge</button></span>';
      row.querySelector('.t').textContent = a.path.split('/').pop();
      row.querySelector('.rule').textContent = `${a.path} (${a.n})  +  ${b.path} (${b.n})`;
      merges.appendChild(row);
    });
    wrap.appendChild(libSection('Project merges',
      'a path is evidence, not identity - authored (ADR 3), never derived', merges));

    const pins = el('div');
    pins.appendChild(libEmpty('Nothing pinned',
      'A pin adds a conversation to a collection the query would not have found. It is ' +
      'the escape hatch that lets the rule stay simple.'));
    wrap.appendChild(libSection('Pinned', 'kept by hand', pins));

    col.appendChild(wrap);
    panes.appendChild(col);
    panes.appendChild(previewCol());
    return panes;
  }

  /* Two projects whose last path segment matches are probably one project that moved.
     Suggested, never applied: the merge is an authored fact (ADR 3), and guessing it into
     the index would make a derived column depend on a judgement call. */
  function mergeCandidates() {
    const by = new Map();
    PROJECTS.forEach((p) => {
      const leaf = p.path.split('/').pop();
      if (!by.has(leaf)) by.set(leaf, []);
      by.get(leaf).push(p);
    });
    return [...by.values()].filter((v) => v.length === 2)
      .map((v) => v.slice().sort((a, b) => b.n - a.n));
  }

  /* ============================================ annotation overlay ======== */

  const NOTE_W = 264;
  const MARGIN = 10;
  const GAP = 9;

  const hits = (a, b) =>
    !(a.x + a.w + GAP < b.x || b.x + b.w + GAP < a.x || a.y + a.h + GAP < b.y || b.y + b.h + GAP < a.y);

  function drawAnnotations() {
    const layer = $('anno');
    layer.textContent = '';
    layer.hidden = !state.annotations;
    if (!state.annotations) return;

    const present = ANNOTATIONS.map((a) => ({ a, node: document.querySelector(a.sel) })).filter(
      (x) => x.node && x.node.getBoundingClientRect().width > 0
    );

    const vw = window.innerWidth;
    const vh = window.innerHeight;

    // Leaders go under the notes, so the tie to a displaced note stays visible.
    const NS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('width', vw);
    svg.setAttribute('height', vh);
    svg.style.cssText = 'position:absolute;inset:0;pointer-events:none';
    layer.appendChild(svg);

    const placed = [];

    present.forEach(({ a, node }, idx) => {
      const r = node.getBoundingClientRect();

      const halo = el('div', 'halo');
      halo.style.cssText =
        `left:${r.left - 3}px;top:${r.top - 3}px;width:${r.width + 6}px;height:${r.height + 6}px`;
      layer.appendChild(halo);

      const pin = el('div', 'pin');
      pin.textContent = String(idx + 1);
      pin.style.left = r.left + 'px';
      pin.style.top = r.top + 'px';
      pin.style.animationDelay = idx * 35 + 'ms';
      layer.appendChild(pin);

      const note = el('div', 'note');
      note.innerHTML = '<b></b><p></p>';
      note.querySelector('b').textContent = `${idx + 1}. ${a.t}`;
      note.querySelector('p').textContent = a.d;
      note.style.animationDelay = idx * 35 + 'ms';
      layer.appendChild(note);

      const h = note.offsetHeight;
      const clampX = (v) => Math.max(MARGIN, Math.min(v, vw - NOTE_W - MARGIN));
      const clampY = (v) => Math.max(MARGIN, Math.min(v, vh - h - MARGIN));

      let x = clampX(
        a.side === 'left' ? r.left - NOTE_W - 18 : a.side === 'right' ? r.right + 18 : r.left
      );
      let y = clampY(a.side === 'below' ? r.bottom + 16 : r.top - 6);

      // Greedy de-collision: slide down, and on running out of column, start a new
      // one. Bounded, because an unplaceable note should still be drawn somewhere.
      for (let guard = 0; guard < 240 && placed.some((p) => hits({ x, y, w: NOTE_W, h }, p)); guard++) {
        y += 10;
        if (y + h > vh - MARGIN) {
          y = MARGIN;
          x = clampX(x > vw / 2 ? x - (NOTE_W + GAP) : x + (NOTE_W + GAP));
        }
      }

      note.style.left = x + 'px';
      note.style.top = y + 'px';
      placed.push({ x, y, w: NOTE_W, h });

      // Leader from the pin to the nearest edge midpoint of where the note landed.
      const cx = r.left;
      const cy = r.top;
      const tx = cx < x ? x : cx > x + NOTE_W ? x + NOTE_W : cx;
      const ty = cy < y ? y : cy > y + h ? y + h : cy;
      const line = document.createElementNS(NS, 'line');
      line.setAttribute('x1', cx);
      line.setAttribute('y1', cy);
      line.setAttribute('x2', tx);
      line.setAttribute('y2', ty);
      line.setAttribute('stroke', 'var(--hit)');
      line.setAttribute('stroke-width', '1');
      line.setAttribute('stroke-dasharray', '3 3');
      line.setAttribute('opacity', '0.55');
      svg.appendChild(line);
    });

    const hint = el('div', 'anno-hint');
    hint.textContent = `${present.length} notes on this view · press N or click “notes” to hide`;
    layer.appendChild(hint);
  }

  /* ===================================================== minimap =========== */

  function syncViewport() {
    const body = $('pv-body');
    const view = $('mm-view');
    const mm = $('mm');
    if (!body || !view || !mm) return;
    const h = mm.clientHeight;
    const frac = body.clientHeight / Math.max(body.scrollHeight, 1);
    const off = body.scrollTop / Math.max(body.scrollHeight, 1);
    view.style.height = Math.max(14, frac * h) + 'px';
    view.style.top = off * h + 'px';
    mm.setAttribute('aria-valuenow', Math.round(off * 100));
  }

  function wireMinimap() {
    const mm = $('mm');
    const body = $('pv-body');
    if (!mm || !body) return;

    const scrub = (clientY) => {
      const r = mm.getBoundingClientRect();
      const f = Math.min(1, Math.max(0, (clientY - r.top) / r.height));
      body.scrollTop = f * (body.scrollHeight - body.clientHeight);
    };

    let dragging = false;
    mm.addEventListener('pointerdown', (e) => {
      dragging = true;
      mm.setPointerCapture(e.pointerId);
      scrub(e.clientY);
    });
    mm.addEventListener('pointermove', (e) => dragging && scrub(e.clientY));
    mm.addEventListener('pointerup', (e) => {
      dragging = false;
      mm.releasePointerCapture(e.pointerId);
    });
    mm.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown') { body.scrollTop += 48; e.preventDefault(); }
      if (e.key === 'ArrowUp') { body.scrollTop -= 48; e.preventDefault(); }
    });
    body.addEventListener('scroll', syncViewport);
    requestAnimationFrame(syncViewport);
  }

  /* ===================================================== render =========== */

  function render() {
    refreshMatches();
    document.querySelectorAll('#views button').forEach((b) =>
      b.setAttribute('aria-pressed', b.dataset.view === state.view ? 'true' : 'false')
    );
    // Every facet narrows the conversation list and nothing else. Library is a list of
    // rules, pins and merges, so the rail and the query line are hidden there rather
    // than drawn inert — which is what Sittings used to do, right down to showing topic
    // counts that moved when you filtered a list the view was not displaying.
    const lib = state.view === 'library';
    $('sbar').hidden = lib;
    $('side').hidden = lib;
    document.querySelector('.body').classList.toggle('solo', lib);

    drawQuery();
    drawGroupControl();
    drawStat();
    if (!lib) drawSidebar();
    drawFooter();

    // render() rebuilds #main from scratch, so both scrollers reset to the top. Clicking
    // a line 4,000px into a conversation threw you back to its first message — which is
    // what read as the pane flashing: it was not an animation, it was the whole thing
    // being rebuilt and repainted at scroll 0.
    const keep = { pv: $('pv-body'), list: $('list') };
    const at = { pv: keep.pv ? keep.pv.scrollTop : 0, list: keep.list ? keep.list.scrollTop : 0 };
    const same = state.selected;

    const main = $('main');
    main.textContent = '';
    main.appendChild(state.view === 'library' ? viewLibrary() : viewSearch());

    // Restored only when the pane is still showing the same conversation; opening a
    // different one should start at its beginning.
    const pv = $('pv-body');
    if (pv && at.pv && same === state.selected) pv.scrollTop = at.pv;
    const list = $('list');
    if (list && at.list) list.scrollTop = at.list;

    // A chip anywhere is the same filter as a chip in the rail, not a second mechanism —
    // TUI-DESIGN §5, and `me9.16` for the shipped version of the same rule. Wired once
    // over the whole subtree so a new surface cannot forget to do it.
    main.querySelectorAll('[data-topic]').forEach((b) =>
      b.addEventListener('click', (e) => {
        e.stopPropagation();
        toggleTopic(b.dataset.topic);
        render();
      })
    );
    main.querySelectorAll('[data-dir]').forEach((b) =>
      b.addEventListener('click', (e) => {
        e.stopPropagation();
        toggleDir(b.dataset.dir);
        render();
      })
    );

    // All three views now end in the same drawer, so its controls are wired once by
    // presence rather than by view name. Keying this on `view === 'search'` is what left
    // the fidelity buttons and the minimap dead the first time the drawer appeared
    // somewhere else.
    if (main.querySelector('.col.preview')) {
      wireMinimap();
      document.querySelectorAll('#zoom button[data-zoom]').forEach((b) =>
        b.addEventListener('click', () => {
          state.fidelity = Object.assign({}, PRESETS[b.dataset.zoom]);
          render();
        })
      );
      // Chip body: cycle. Shift reverses, so no transition costs two clicks.
      document.querySelectorAll('#fid button[data-kind]').forEach((b) =>
        b.addEventListener('click', (e) => {
          cycleKind(b.dataset.kind, e.shiftKey);
          state.zoomTouched = true;
          state.overrides.clear();
          render();
        })
      );
      // Dot: visibility, straight there and back.
      document.querySelectorAll('#fid button[data-vis]').forEach((b) =>
        b.addEventListener('click', (e) => {
          e.stopPropagation();
          toggleKind(b.dataset.vis);
          state.zoomTouched = true;
          state.overrides.clear();
          render();
        })
      );

      const fold = document.querySelector('[data-tagfold]');
      if (fold) fold.addEventListener('click', () => { state.pvTopics = !state.pvTopics; render(); });

      document.querySelectorAll('#fid-all button').forEach((b) =>
        b.addEventListener('click', () => { state.overrides.clear(); render(); })
      );
    }
    drawAnnotations();
  }

  /* ===================================================== wiring =========== */

  function wire() {
    document.querySelectorAll('#views button').forEach((b) =>
      b.addEventListener('click', () => {
        state.view = b.dataset.view;
        render();
      })
    );

    $('btn-offpath').addEventListener('click', (e) => {
      state.showOffPath = !state.showOffPath;
      e.currentTarget.setAttribute('aria-pressed', state.showOffPath ? 'true' : 'false');
      render();
    });

    $('btn-anno').addEventListener('click', toggleAnnotations);

    $('btn-theme').addEventListener('click', (e) => {
      const light = document.documentElement.classList.toggle('light');
      // Two classes for one theme, which is not duplication: `light` is what
      // styles.css and the annotation layer have always keyed off, and `theme-light`
      // is what a direction in directions.css keys off, because a direction has to be
      // able to scope to a subtree. directions.html renders both themes at once on
      // exactly that difference.
      root.classList.toggle('theme-light', light);
      root.classList.toggle('theme-dark', !light);
      e.currentTarget.textContent = light ? 'dark' : 'light';
      if (state.annotations) drawAnnotations();
    });

    document.addEventListener('keydown', (e) => {
      if (e.key === 'n' || e.key === 'N') toggleAnnotations();
      if (e.key === 'Escape' && state.annotations) toggleAnnotations();
      if (state.view !== 'search') return;
      const v = visible();
      const i = v.findIndex((c) => c.id === state.selected);
      if (e.key === 'ArrowDown' || e.key === 'j') {
        selectConversation(v[Math.min(v.length - 1, i + 1)].id);
        e.preventDefault();
      }
      if (e.key === 'ArrowUp' || e.key === 'k') {
        selectConversation(v[Math.max(0, i - 1)].id);
        e.preventDefault();
      }
    });

    let t;
    window.addEventListener('resize', () => {
      syncViewport();
      if (!state.annotations) return;
      clearTimeout(t);
      t = setTimeout(drawAnnotations, 120);
    });
  }

  function toggleAnnotations() {
    state.annotations = !state.annotations;
    $('btn-anno').setAttribute('aria-pressed', state.annotations ? 'true' : 'false');
    drawAnnotations();
  }

  if (CONVERSATIONS.length) {
    state.selected = CONVERSATIONS[0].id;
    state.fidelity = defaultZoomFor(CONVERSATIONS[0]);
  }

  /* The gallery renders the same functions this app does, against the same state, so a
     component cannot look one way there and another way here. Duplicating the markup
     into a second file is the drift this whole session kept finding — two numbers for
     one measurement — and a gallery that has drifted is worse than none, because you
     make polish decisions against something that is not what ships. */
  window.CS_UI = {
    state, render,
    conversationRow, ribbon, block, breakRule, cutRule, segmentize: null,
    groupSection, projectBand, runBand, topicBand, sourceBand, untaggedBand,
    workSummary, sparkline, runsOf, statsOf, previewCol,
    KINDS, LEVELS, LEVEL_WORD, PRESETS, GROUPINGS, ACT_ORDER,
    el, elidePath, dayLabel, clockOf, diffBadge,
  };

  // The app shell is absent in the gallery, so booting it would throw on the first
  // getElementById. Everything above is already exported by then.
  if ($('main')) {
    applyDirection();
    render();
    wire();
  }
})();
