/* Organising variants, second pass — rebuilt from feedback.
 *
 * Changes from the first pass, and why:
 *   · Views switch in place (Notion-style) instead of stacking six panels down a page.
 *   · One preview drawer, shared by every view. Clicking a conversation anywhere opens
 *     the same drawer — the list is the variable, the reader is not.
 *   · Inbox dropped: it did not earn its place.
 *   · Projects, cwd, lineages and files merged. They were four names for one idea, and
 *     files only make sense scoped to a directory, not across the whole corpus.
 *   · Timeline gained zoom and a scrub range.
 *   · Topics grouped, and split by facet — subject versus mode.
 */
(function () {
  'use strict';

  const D = window.REAL_DATA;
  if (!D) { document.getElementById('nodata').hidden = false; return; }

  const el = (t, c) => { const n = document.createElement(t); if (c) n.className = c; return n; };
  const esc = (s) => String(s).replace(/[<>&]/g, (m) => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;' }[m]));
  const convs = D.conversations;
  const byId = new Map(convs.map((c) => [c.id, c]));
  const title = (c) => c.title || '(untitled)';
  const day = (ms) => (ms ? new Date(ms).toISOString().slice(0, 10) : '—');
  const shortSrc = (s) => (s === 'chatgpt-export' ? 'chatgpt' : s);
  const badge = (src) => (typeof sourceBadge === 'function'
    ? sourceBadge(shortSrc(src))
    : `<span class="badge">${esc(shortSrc(src))}</span>`);

  const state = { view: 'topics', selected: convs[0] && convs[0].id, facets: new Set(), zoom: null };

  /* ------------------------------------------------------- shared drawer --- */

  function drawDrawer() {
    const host = document.getElementById('drawer');
    const c = byId.get(state.selected);
    if (!c) { host.innerHTML = '<div class="dr-empty">Select a conversation</div>'; return; }

    const kinds = { p: 'prose', r: 'reasoning', c: 'tool', o: 'tool' };
    const groups = (D.topics || []).filter((t) => t.members.includes(c.id));
    host.innerHTML =
      `<div class="dr-h">${badge(c.source)}<span class="dr-t"></span></div>` +
      `<div class="dr-m">${c.msg_count} msgs · ${c.user_turns} turns · ${day(c.ended_at)}` +
      (c.cwd ? ` · ${esc(c.cwd.split('/').slice(-2).join('/'))}` : '') + '</div>' +
      (groups.length
        ? `<div class="dr-tags">${groups.map((t) => `<span class="chip sm">${esc(t.name)}</span>`).join('')}</div>`
        : '<div class="dr-tags"><span class="chip sm ghost">no topic</span></div>') +
      '<div class="dr-body"></div>';
    host.querySelector('.dr-t').textContent = title(c);

    const body = host.querySelector('.dr-body');
    c.msgs.slice(0, 60).forEach((m) => {
      if (m.k === 'o' && !m.e) return;
      const k = m.k === 'p' ? (m.r === 'u' ? 'user' : 'agent') : kinds[m.k];
      const b = el('div', 'dr-blk ' + (m.e ? 'err' : k));
      const txt = (m.x || '').slice(0, m.k === 'p' ? 400 : 90);
      b.innerHTML = '<i class="g"></i><span class="x"></span>';
      b.querySelector('.x').textContent = txt || '…';
      body.appendChild(b);
    });
  }

  const pick = (id) => { state.selected = id; drawDrawer(); paintSelection(); };

  function paintSelection() {
    document.querySelectorAll('[data-cid]').forEach((n) =>
      n.classList.toggle('on', n.dataset.cid === state.selected));
  }

  function convRow(c, meta) {
    const r = el('button', 'crow');
    r.type = 'button';
    r.dataset.cid = c.id;
    r.innerHTML = badge(c.source) + '<span class="t"></span>' +
                  `<span class="m">${esc(meta || (c.msg_count + ' msgs'))}</span>`;
    r.querySelector('.t').textContent = title(c);
    r.addEventListener('click', () => pick(c.id));
    return r;
  }

  /* -------------------------------------------------------- view: topics --- */
  /* Grouped, and split by facet. "Prose and editing" and "Technical writing and docs"
   * read as duplicates in a flat list yet share only 0.13 Jaccard on the corpus — one
   * is a mode, one is a subject. Separating the axes is what stops them competing. */

  function viewTopics(host) {
    const topics = D.topics || [];
    const sel = state.facets;
    const live = (t) => convs.filter((c) =>
      t.members.includes(c.id) &&
      [...sel].every((n) => (topics.find((x) => x.name === n) || { members: [] }).members.includes(c.id))).length;

    const wrap = el('div', 'two');
    const l = el('div', 'l'); const r = el('div', 'r');

    ['subject', 'mode'].forEach((facet) => {
      const fs = topics.filter((t) => t.facet === facet);
      if (!fs.length) return;
      l.appendChild(Object.assign(el('p', 'lbl'), {
        textContent: facet === 'subject' ? 'subject — what it was about' : 'mode — how you were working',
      }));
      const groups = {};
      fs.forEach((t) => (groups[t.group] = groups[t.group] || []).push(t));
      Object.entries(groups).forEach(([g, list]) => {
        const gh = el('div', 'grp');
        gh.innerHTML = `<span class="gname">${esc(g)}</span>`;
        list.sort((a, b) => b.members.length - a.members.length).forEach((t) => {
          const n = live(t);
          const ch = el('button', 'chip' + (sel.has(t.name) ? ' on' : '') + (n ? '' : ' dim'));
          ch.innerHTML = `${esc(t.name)} <span class="c">${n}</span>`;
          ch.addEventListener('click', () => {
            sel.has(t.name) ? sel.delete(t.name) : sel.add(t.name);
            render();
          });
          gh.appendChild(ch);
        });
        l.appendChild(gh);
      });
    });

    const hits = convs.filter((c) =>
      [...sel].every((n) => (topics.find((x) => x.name === n) || { members: [] }).members.includes(c.id)));
    r.innerHTML = `<p class="lbl">${sel.size ? [...sel].join(' AND ') : 'everything'} · ${hits.length}</p>`;
    hits.slice(0, 40).forEach((c) => r.appendChild(convRow(c, day(c.ended_at))));
    if (!hits.length) r.innerHTML += '<div class="note-sm">no conversation carries all of those</div>';

    wrap.appendChild(l); wrap.appendChild(r); host.appendChild(wrap);
  }

  /* ------------------------------------------------------ view: projects --- */
  /* cwd, lineages and files were three names for one thing. A project is a directory;
   * a lineage is a stretch of work in it; files are what that work touched. Files only
   * make sense inside this scope — across the whole corpus a bare `README.md` matches
   * a pitch deck and a hackathon, which is what the first pass showed. */

  function viewProjects(host) {
    const byCwd = {};
    convs.filter((c) => c.cwd).forEach((c) => (byCwd[c.cwd] = byCwd[c.cwd] || []).push(c));
    const dirs = Object.entries(byCwd).sort((a, b) => b[1].length - a[1].length);

    const wrap = el('div', 'two');
    const l = el('div', 'l'); const r = el('div', 'r');
    l.innerHTML = `<p class="lbl">directories · ${dirs.length} · covers ${convs.filter((c) => c.cwd).length} of ${convs.length}</p>`;

    const fill = (cwd, list) => {
      r.innerHTML = `<p class="lbl">${esc(cwd.split('/').slice(-2).join('/'))} · ${list.length} conversations</p>`;
      const lins = (D.lineages || []).filter((L) => L.cwd === cwd);
      if (lins.length) {
        const lw = el('div', 'note-sm');
        lw.innerHTML = 'lineages — stretches of work, split on gaps over three days: ' +
          lins.map((L) => `<b>${L.ids.length} convs / ${Math.max(1, Math.round((L.end - L.start) / 86400000))}d</b>`).join(' · ');
        r.appendChild(lw);
      }
      const files = {};
      list.forEach((c) => (c.files || []).forEach((f) => (files[f] = (files[f] || 0) + 1)));
      const top = Object.entries(files).sort((a, b) => b[1] - a[1]).slice(0, 10);
      if (top.length) {
        const fw = el('div', 'files');
        fw.innerHTML = '<p class="lbl">files touched in this directory</p>' +
          top.map(([f, n]) => `<span class="chip sm">${esc(f)} <span class="c">${n}</span></span>`).join('');
        r.appendChild(fw);
      }
      list.sort((a, b) => (b.ended_at || 0) - (a.ended_at || 0))
          .slice(0, 25).forEach((c) => r.appendChild(convRow(c, day(c.ended_at))));
      paintSelection();
    };

    dirs.slice(0, 16).forEach(([cwd, list], i) => {
      const b = el('button', 'dirline' + (i === 0 ? ' on' : ''));
      const nf = new Set(list.flatMap((c) => c.files || [])).size;
      b.innerHTML = `<span class="p">${esc(cwd.split('/').slice(-2).join('/'))}</span>` +
                    `<span class="c">${list.length}c · ${nf}f</span>`;
      b.addEventListener('click', () => {
        l.querySelectorAll('.dirline').forEach((x) => x.classList.remove('on'));
        b.classList.add('on'); fill(cwd, list);
      });
      l.appendChild(b);
    });
    wrap.appendChild(l); wrap.appendChild(r); host.appendChild(wrap);
    if (dirs.length) fill(dirs[0][0], dirs[0][1]);
  }

  /* ------------------------------------------------------ view: timeline --- */

  function viewTimeline(host) {
    const withTs = convs.filter((c) => c.ended_at).sort((a, b) => a.ended_at - b.ended_at);
    const full = [withTs[0].ended_at, withTs[withTs.length - 1].ended_at];
    const [lo, hi] = state.zoom || full;
    const inRange = withTs.filter((c) => c.ended_at >= lo && c.ended_at <= hi);
    const sources = [...new Set(withTs.map((c) => c.source))];

    const bar = el('div', 'tbar2');
    bar.innerHTML =
      `<span class="lbl" style="margin:0">${day(lo)} → ${day(hi)} · ${inRange.length} conversations</span>` +
      '<span style="flex:1"></span>' +
      ['30d', '90d', '1y', 'all'].map((z) => `<button class="mini" data-z="${z}">${z}</button>`).join('');
    host.appendChild(bar);
    bar.querySelectorAll('[data-z]').forEach((b) => b.addEventListener('click', () => {
      const d = { '30d': 30, '90d': 90, '1y': 365 }[b.dataset.z];
      state.zoom = d ? [full[1] - d * 86400000, full[1]] : null;
      render();
    }));

    const tl = el('div', 'tl2');
    sources.forEach((s) => {
      const row = el('div', 'tl-row');
      row.innerHTML = `<span class="name">${esc(shortSrc(s))}</span>`;
      const track = el('div', 'track');
      inRange.filter((c) => c.source === s).forEach((c) => {
        const m = el('button', 'tl-mark');
        m.dataset.cid = c.id;
        m.style.left = (((c.ended_at - lo) / Math.max(hi - lo, 1)) * 100) + '%';
        m.style.height = Math.max(5, Math.min(20, Math.log10(c.msg_count + 1) * 8)) + 'px';
        m.title = title(c);
        m.addEventListener('click', () => pick(c.id));
        track.appendChild(m);
      });
      row.appendChild(track);
      tl.appendChild(row);
    });
    host.appendChild(tl);

    // Scrub: drag across the overview to set the range.
    const ov = el('div', 'scrub');
    ov.innerHTML = '<span class="lbl" style="margin:0 0 4px">drag to scrub the full range</span>';
    const track = el('div', 'scrub-t');
    withTs.forEach((c) => {
      const m = el('i');
      m.style.left = (((c.ended_at - full[0]) / Math.max(full[1] - full[0], 1)) * 100) + '%';
      track.appendChild(m);
    });
    const sel = el('div', 'scrub-sel');
    sel.style.left = (((lo - full[0]) / (full[1] - full[0])) * 100) + '%';
    sel.style.width = (((hi - lo) / (full[1] - full[0])) * 100) + '%';
    track.appendChild(sel);
    let drag = null;
    const at = (e) => {
      const r = track.getBoundingClientRect();
      return full[0] + ((e.clientX - r.left) / r.width) * (full[1] - full[0]);
    };
    track.addEventListener('pointerdown', (e) => { drag = at(e); track.setPointerCapture(e.pointerId); });
    track.addEventListener('pointermove', (e) => {
      if (drag == null) return;
      const b = at(e);
      state.zoom = [Math.min(drag, b), Math.max(drag, b)];
      sel.style.left = (((Math.min(drag, b) - full[0]) / (full[1] - full[0])) * 100) + '%';
      sel.style.width = ((Math.abs(b - drag) / (full[1] - full[0])) * 100) + '%';
    });
    track.addEventListener('pointerup', () => { drag = null; render(); });
    ov.appendChild(track);
    host.appendChild(ov);
    paintSelection();
  }

  /* ----------------------------------------------------- view: untagged --- */

  function viewUntagged(host) {
    const ids = new Set(D.coverage.untagged_ids || []);
    const list = convs.filter((c) => ids.has(c.id));
    host.innerHTML =
      `<p class="lbl">${list.length} of ${convs.length} here match no topic — 37% on the full corpus</p>` +
      '<div class="note-sm">Not junk: short one-off questions, ordinary vocabulary, and domains ' +
      'no seed covers. At this scale it is the largest single region in the archive.</div>';
    list.forEach((c) => host.appendChild(convRow(c, day(c.ended_at))));
    paintSelection();
  }

  /* ------------------------------------------------------------- render --- */

  const VIEWS = [
    ['topics', 'Topics', viewTopics],
    ['projects', 'Projects', viewProjects],
    ['timeline', 'Timeline', viewTimeline],
    ['untagged', 'Untagged', viewUntagged],
  ];

  function render() {
    const tabs = document.getElementById('tabs');
    tabs.innerHTML = VIEWS.map(([k, n]) =>
      `<button class="vtab${state.view === k ? ' on' : ''}" data-v="${k}">${n}</button>`).join('');
    tabs.querySelectorAll('[data-v]').forEach((b) =>
      b.addEventListener('click', () => { state.view = b.dataset.v; render(); }));

    const host = document.getElementById('main');
    host.innerHTML = '';
    (VIEWS.find((v) => v[0] === state.view) || VIEWS[0])[2](host);
    drawDrawer();
    paintSelection();
  }

  render();
})();
