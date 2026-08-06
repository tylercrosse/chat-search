/* chat-search — every visual direction, on the two marks that break first.
 *
 * DESIGN-BRIEF.md hands the look of this prototype to a designer and asks for several
 * distinct directions rather than one refinement, with the row and the ribbon shown at
 * real size in each — because those are where a purely visual pass breaks without
 * noticing. This page is that ask, answered in the prototype's own tokens. The last
 * two are not answers to that brief: they are ports of Gruvbox and Solarized, here for
 * the same reason the others are, which is that a table of ratios is not a picture.
 *
 * Three rules, and they are the whole design of the file:
 *
 *   1. It renders window.CS_UI and window.CS_FIXTURES — the app's own row and ribbon,
 *      the gallery's own conversations. A specimen here cannot look like something
 *      that does not ship, and cannot disagree with the gallery about what a row is.
 *   2. Both themes render in one document, as sibling subtrees carrying `.theme-dark`
 *      and `.theme-light`. The gallery needs two iframes for this because the theme
 *      lives on `:root`; the token layer's theme classes are what removed that.
 *   3. Every number in the table is measured off the rendered page, not read from
 *      directions.css and not copied from palette.py. Both of those can be right about
 *      a token and wrong about what a browser did with it — a row height in particular
 *      is a line box's opinion, not a sum of the padding you asked for.
 *
 * Plain script, no modules, no build, opens from disk. Same as the rest.
 */
(function () {
  'use strict';

  const UI = window.CS_UI;
  const F = window.CS_FIXTURES;
  const $ = (id) => document.getElementById(id);
  const el = (tag, cls) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    return n;
  };

  /* Named, because "the second one" is not something two people can agree about a week
     later. The sentence is the bet each direction is making, not a description of it,
     and the third field is where it came from — a port is not a candidate answer to
     the brief and the page should not call it one. */
  const DIRECTIONS = [
    ['terminal', 'The incumbent, and the control. System faces, teal and slate, ' +
      'hairlines at low contrast, radii that grew ad hoc. It declares no tokens of ' +
      'its own — it is styles.css as it stands, so every other row is measured ' +
      'against what actually ships rather than against a restatement of it.',
      'control'],
    ['paper', 'Ink on stock rather than phosphor on glass. A warm ground, a text face ' +
      'for the two lines of the row that are prose, square corners, and rules ' +
      'withdrawn until space is doing most of the separating. The bet is that an ' +
      'archive reads more like a set of documents than like a process listing.',
      'candidate'],
    ['blueprint', 'The terminal register taken seriously instead of apologised for. ' +
      'Everything in the mono face, every corner square, and the hairlines raised ' +
      'rather than hidden so the grid the row already is becomes visible. The one ' +
      'direction that buys rows-per-screen rather than spending it.',
      'candidate'],
    ['ink', 'The argument that this product has no business being colourful. Chrome ' +
      'goes neutral, corners soften, rules all but vanish and separation comes from ' +
      'the ground a thing sits on. What colour remains is the reserved vocabulary — ' +
      'amber for a match, red for a failure, blue for selection — and the four kinds, ' +
      'which keep only enough hue to be a second channel.',
      'candidate'],
    ['gruvbox-derived', 'Gruvbox, which somebody asked for by name. Its grounds and ' +
      'its accents are published values; the eight tokens the ramp and the AA floor ' +
      'fence are its hues at lightnesses palette.py solved, because the published ' +
      'ones miss. A nudge rather than a rebuild — its dark ramp is already even at ' +
      '1.77x 1.78x 1.82x, and no band moves more than 14 points of lightness.',
      'ported'],
    ['solarized-derived', 'Solarized, and this one is a rebuild. No assignment of its ' +
      'sixteen colours makes an even ramp at all, so its brightest kind leaves the ' +
      'range entirely — base1 at L 60% comes back at L 87% — and the tier it ' +
      'designates for secondary content, at 2.42:1 on base02, comes back brighter ' +
      'than its own body text. Low contrast is what Solarized is.',
      'ported'],
  ];

  /* ------------------------------------------------------------ measuring -- */

  const toLinear = (c) => {
    c /= 255;
    return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  };
  const luminance = ([r, g, b]) =>
    0.2126 * toLinear(r) + 0.7152 * toLinear(g) + 0.0722 * toLinear(b);

  function rgbOf(value) {
    const s = String(value).trim();
    const hex = s.replace('#', '');
    if (/^[0-9a-f]{6}$/i.test(hex)) {
      return [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16));
    }
    if (/^[0-9a-f]{3}$/i.test(hex)) {
      return hex.split('').map((c) => parseInt(c + c, 16));
    }
    const m = s.match(/rgba?\(([^)]+)\)/);
    return m ? m[1].split(',').slice(0, 3).map((n) => parseFloat(n)) : null;
  }

  function contrast(a, b) {
    const x = rgbOf(a);
    const y = rgbOf(b);
    if (!x || !y) return NaN;
    const la = luminance(x);
    const lb = luminance(y);
    return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
  }

  /* Everything the table says about one pane, read back off the pane. */
  function measure(pane) {
    const cs = getComputedStyle(pane);
    const tok = (n) => cs.getPropertyValue(n).trim();

    const kinds = ['tool', 'reason', 'user', 'agent'];
    const ramp = kinds.map((k) => contrast(tok('--k-' + k), tok('--map-bg')));
    const steps = ramp.slice(1).map((r, i) => r / ramp[i]);

    // The row that decides density: three lines, no query, which is what browsing
    // looks like. Measured rather than derived, because line boxes round and the
    // ribbon's 18px can end up taller than the text it sits beside.
    const row = pane.querySelector('.row');
    const height = row ? row.getBoundingClientRect().height : NaN;

    return {
      height,
      ramp,
      steps,
      // The tier is checked on both grounds it lands on. The drawer is the one that
      // was missed the first time: --panel is darker than --bg in a light theme, and
      // most of this tier's text is in the transcript, which sits on the drawer.
      quietPage: contrast(tok('--ink-3'), tok('--bg')),
      quietDrawer: contrast(tok('--ink-3'), tok('--panel')),
      face: tok('--ui').split(',')[0].replace(/"/g, ''),
      body: tok('--fs-body'),
    };
  }

  /* ------------------------------------------------------------ specimens -- */

  /* The row's third line exists only when there is a query, and `hasQuery()` reads
     state. The gallery already owns the borrow-and-restore for that, so this page uses
     it rather than keeping a second one honest. */
  const withQuery = F.withQuery;

  const cap = (text) => {
    const c = el('div', 'vd-cap');
    c.textContent = text;
    return c;
  };

  function rowStack() {
    const list = el('div', 'list');
    const busy = F.conv('u ' + F.rep('c:run o', 8) + ' c:change o ' +
                        F.rep('c:run o', 6) + ' a');
    const hit = F.conv('u ' + F.rep('c o', 6) + ' c* o a ' + F.rep('c o', 5) + ' a');
    hit.title = 'ymd() renders UTC instead of the local day';
    const chat = F.conv(F.rep('a u', 4), {
      topics: [], src: 'chatgpt', model: 'gpt-5.4', dir: '—', project: null, files: [],
    });
    chat.title = 'Is a civil day always 86,400 seconds?';

    // Order matters: the first .row is the one measure() reads, and density is a
    // question about browsing, not about a row that happens to carry a match.
    withQuery(false, () => {
      list.appendChild(UI.conversationRow(busy));
      const sel = UI.conversationRow(busy);
      sel.setAttribute('aria-selected', 'true');
      list.appendChild(sel);
      list.appendChild(UI.conversationRow(chat));
    });
    withQuery(true, () => list.appendChild(UI.conversationRow(hit)));
    return list;
  }

  function ribbonStack() {
    const d = el('div', 'vd-ribs');
    const add = (label, node) => {
      d.appendChild(cap(label));
      d.appendChild(node);
    };
    // The four kinds alone is the specimen the first finding lives or dies on: if two
    // of these read as one colour, the direction has lost it.
    add('you · agent · reasoning · tools, each alone — 13.0 / 7.2 / 4.0 / 2.2 : 1',
        F.rib(F.conv(F.rep('u', 14))));
    add('', F.rib(F.conv(F.rep('a', 14))));
    add('', F.rib(F.conv(F.rep('r', 14))));
    add('', F.rib(F.conv(F.rep('c o', 14))));
    add('a real agentic shape — a steer, then the run it caused',
        F.rib(F.conv('u ' + F.rep('c o', 9) + ' a u ' + F.rep('c o', 7) + ' a')));
    add('acts inside the tool band — look, run, change',
        F.rib(F.conv('u ' + F.rep('c:look o', 4) + ' ' + F.rep('c:run o', 6) + ' ' +
                     F.rep('c:change o', 3) + ' ' + F.rep('c:run o', 5) + ' a')));
    add('overlays — match ticks, a >4h pause, a compaction, a failure, a question',
        F.rib(F.conv('u c o c* o a? c o~ c o! a c o u# a c o c* o a'), { q: true }));
    add('length is the drawn width — 10, 91, 344, 2553 messages',
        F.rib(F.conv(F.rep('c o', 5), { n: 10 })));
    [91, 344, 2553].forEach((n) => add('', F.rib(
      F.conv(F.rep('c o', Math.max(2, Math.round(n / 2))), { n }))));
    return d;
  }

  function pane(id, theme) {
    const p = el('div', `vd-pane dir-${id} theme-${theme}`);
    const h = el('div', 'vd-pane-h');
    h.textContent = theme;
    p.appendChild(h);
    p.appendChild(cap('the row — 706px, the real list column at a 1560px window'));
    p.appendChild(rowStack());
    p.appendChild(cap('the ribbon — 200px, the real grid track'));
    p.appendChild(ribbonStack());
    return p;
  }

  /* ---------------------------------------------------------------- table -- */

  const cell = (row, text, cls) => {
    const c = el('td', cls);
    c.textContent = text;
    row.appendChild(c);
    return c;
  };

  const verdict = (ok) => (ok ? 'holds' : 'BREAKS');

  function summary(readings) {
    const t = el('table', 'vd-tbl');
    const head = el('tr');
    ['direction', 'theme', 'row', 'rows / 800px', 'kind ramp, steps',
     'quiet tier — page / drawer', 'holds?'].forEach((h) => {
      const th = el('th');
      th.textContent = h;
      head.appendChild(th);
    });
    t.appendChild(head);

    const control = readings.find((r) => r.id === 'terminal' && r.theme === 'dark');

    readings.forEach((r) => {
      const m = r.m;
      const perScreen = Math.floor(800 / m.height);
      const controlPerScreen = Math.floor(800 / control.m.height);
      const even = m.steps.every((s) => Math.abs(s - 1.8) < 0.06);
      const quiet = Math.min(m.quietPage, m.quietDrawer) >= 4.5;
      const dense = perScreen >= controlPerScreen;

      const row = el('tr');
      cell(row, r.id, 'vd-name');
      cell(row, r.theme);
      cell(row, m.height.toFixed(1) + 'px');
      const d = perScreen - controlPerScreen;
      cell(row, perScreen + (d === 0 ? '  (=)' : d > 0 ? `  (+${d})` : `  (${d})`),
           dense ? '' : 'vd-bad');
      cell(row, m.ramp.map((x) => x.toFixed(1)).join(' · ') + '   ' +
           m.steps.map((s) => s.toFixed(2) + 'x').join(' '), even ? '' : 'vd-bad');
      cell(row, m.quietPage.toFixed(2) + ' / ' + m.quietDrawer.toFixed(2),
           quiet ? '' : 'vd-bad');
      cell(row, [even && quiet ? 'findings hold' : 'FINDINGS BREAK',
                 dense ? 'density holds' : 'DENSITY DROPS'].join(' · '),
           even && quiet && dense ? 'vd-ok' : 'vd-bad');
      t.appendChild(row);
    });
    return t;
  }

  /* ----------------------------------------------------------------- page -- */

  const body = $('vd-body');
  const readings = [];

  DIRECTIONS.forEach(([id, thesis, role]) => {
    const sec = el('section', 'gal-sec');
    sec.id = id;
    const h = el('div', 'gal-h');
    const title = el('h2');
    title.textContent = id;
    const meta = el('span');
    meta.textContent = role;
    h.appendChild(title);
    h.appendChild(meta);
    sec.appendChild(h);

    const note = el('p', 'gal-note');
    note.textContent = thesis;
    sec.appendChild(note);

    const pair = el('div', 'vd-pair');
    ['dark', 'light'].forEach((theme) => {
      const p = pane(id, theme);
      pair.appendChild(p);
      readings.push({ id, theme, pane: p });
    });
    sec.appendChild(pair);
    body.appendChild(sec);
  });

  // Measured only once every pane is in the document, because a row's height is a
  // fact about layout and there is no layout before that.
  readings.forEach((r) => { r.m = measure(r.pane); });

  const top = el('section', 'gal-sec');
  const th = el('div', 'gal-h');
  const t2 = el('h2');
  t2.textContent = 'what each direction costs';
  const tm = el('span');
  tm.textContent = 'measured off this page';
  th.appendChild(t2);
  th.appendChild(tm);
  top.appendChild(th);

  const tnote = el('p', 'gal-note');
  tnote.innerHTML =
    'Three things are fenced and two of them were bought with measurement. The ' +
    '<b>kind ramp</b> must stay near an even 1.8× per step, because hue is the ' +
    'channel that degrades fastest at the 2px the bands are drawn at and two of the ' +
    'four kinds once sat 1.12:1 apart — the same colour to the eye. The <b>quiet ' +
    'tier</b> carries dates, counts and section labels rather than decoration, and ' +
    'has to clear 4.5:1 on both grounds it lands on; it was 3.64 dark and 2.90 light ' +
    'at 9–11px once. And <b>rows per screen</b> may not drop, since density is what ' +
    'makes an archive of 3,059 conversations scannable at all. ' +
    'Row height is measured from the first browsing row in each pane; 800px stands ' +
    'in for the list viewport at a 1560×900 window, so the count is a comparison ' +
    'between directions rather than a promise about any particular screen.';
  top.appendChild(tnote);
  top.appendChild(summary(readings));
  body.insertBefore(top, body.firstChild);

  const nav = $('vd-nav');
  DIRECTIONS.forEach(([id]) => {
    const a = el('a');
    a.href = '#' + id;
    a.textContent = id;
    nav.appendChild(a);
  });

  const broken = readings.filter((r) =>
    !r.m.steps.every((s) => Math.abs(s - 1.8) < 0.06) ||
    Math.min(r.m.quietPage, r.m.quietDrawer) < 4.5);
  $('vd-count').textContent =
    `${DIRECTIONS.length} directions · ${readings.length} panes · ` +
    (broken.length ? `${broken.length} breaking a fenced measurement`
                   : 'every fenced measurement holds');
})();
