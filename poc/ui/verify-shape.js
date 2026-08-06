/* Does the ribbon draw the shape `cs` sent?
 *
 *     node poc/ui/verify-shape.js            # against poc/ui/real-data.js
 *     node poc/ui/verify-shape.js /tmp/x.js  # against another export
 *
 * The same role `palette.py --verify` plays for the palette: it re-reads what the code
 * actually produces rather than trusting what it was meant to produce, and exits non-zero.
 * The thing worth checking is that the band boundaries on a row are cs's — the ribbon
 * subdivides a tool run where the act changes, and a subdivision that quietly became a
 * boundary of its own would look like a slightly busier ribbon and nothing else.
 *
 * Loads data.js and app.js the way index.html does, into one context with enough of a DOM
 * to get past `el()`. `ribbon()` builds a string, so nothing here needs a real one. It
 * cannot run in CI: a real export is conversation text and is gitignored.
 *
 * What it cannot see, said plainly so it is not mistaken for more than it is. It reads the
 * markup, so it checks *outcomes* and not provenance: put the local run-length encoder
 * back and it still passes, because the bands it would encode are the ones cs decided per
 * message and the two agree by construction. And ±2px of tolerance is about seven messages
 * on the longest conversation in the corpus, so it catches a wrong axis, not a drift of
 * one or two positions.
 */
'use strict';

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const HERE = __dirname;
const REAL = process.argv[2] || path.join(HERE, 'real-data.js');

/* cs's band names in the class names the stylesheets spell — app.js's BAND_CLASS, stated
   again here on purpose. A check that imports the table it is checking cannot fail. */
const CLASS = { user: 'user', agent: 'agent', reasoning: 'reason', tool: 'tool' };

function load(files) {
  const stub = () => ({
    className: '', style: {}, innerHTML: '', textContent: '',
    classList: { add() {}, remove() {}, contains: () => false },
    setAttribute() {}, appendChild() {}, addEventListener() {},
    querySelector: () => stub(), querySelectorAll: () => [],
  });
  const ctx = { console };
  ctx.window = ctx;
  ctx.document = {
    // Null is what stops app.js booting the shell: everything it exports is already
    // assigned by then, which is the same seam gallery.html loads it through.
    getElementById: () => null,
    createElement: stub,
    createDocumentFragment: stub,
    documentElement: stub(),
    addEventListener() {},
  };
  vm.createContext(ctx);
  for (const f of files) vm.runInContext(fs.readFileSync(f, 'utf8'), ctx, { filename: f });
  // `const` at the top of a classic script lands in the context's lexical scope rather
  // than on the global object, so CONVERSATIONS has to be read by evaluating it.
  return { ctx, conversations: vm.runInContext('CONVERSATIONS', ctx) };
}

const bandsIn = (html) => [...html.matchAll(/class="rb-band ([a-z]+)/g)].map((m) => m[1]);
const dedupe = (xs) => xs.filter((x, i) => x !== xs[i - 1]);

const failures = [];
const fail = (msg) => failures.push(msg);

/* ---------------------------------------------------------------- real ---- */

if (!fs.existsSync(REAL)) {
  console.error(`no export at ${REAL} — run \`python3 poc/ui/export.py\` first`);
  process.exit(2);
}

const app = [path.join(HERE, 'data.js'), path.join(HERE, 'app.js')];
const real = load([REAL, ...app]);
const UI = real.ctx.CS_UI;
let drawnTotal = 0;
let bandTotal = 0;

for (const c of real.conversations) {
  if (!c.shape) { fail(`${c.id}: exported without a shape`); continue; }

  // 1. The axis. Run lengths sum to the drawn messages, which is not `msg_count` — the
  //    number the row prints — and getting this wrong is what put two positions on the
  //    axis for every tool call (NOTES §3.27).
  const drawn = c.msgs.filter((m) => m.drawn !== false).length;
  const summed = c.shape.reduce((a, [, n]) => a + n, 0);
  if (drawn !== summed) fail(`${c.id}: shape sums to ${summed}, ${drawn} messages are drawn`);

  // 2. The boundaries. Collapse the act subdivisions and what is left has to be cs's run
  //    sequence exactly — same bands, same order, none merged, none invented.
  const html = UI.ribbon(c, true);
  const drawnBands = bandsIn(html);
  const wire = c.shape.map(([b]) => CLASS[b]);
  const got = dedupe(drawnBands).join(',');
  const want = dedupe(wire).join(',');
  if (got !== want) {
    fail(`${c.id}: bands ${got.slice(0, 90)}\n      cs's runs ${want.slice(0, 90)}`);
  }
  if (drawnBands.length < wire.length) {
    fail(`${c.id}: ${drawnBands.length} bands for ${wire.length} runs — a boundary was lost`);
  }

  // 3. The positions, which is the check with the teeth in it. Every boundary lands where
  //    cs's cumulative run lengths put it *over the drawn count*. Comparing sequences
  //    alone cannot see the axis: measure the same runs against every head-path message
  //    and the colours come out in the same order, only in the wrong places.
  //
  //    TOLERANCE is the 2px floor a one-message steer is given so it stays a visible
  //    separator; a boundary can be pushed that far and no further.
  const TOLERANCE = 2;
  const placed = [...html.matchAll(
    /class="rb-band ([a-z]+)[^"]*"\s*style="left:([\d.]+)px;width:([\d.]+)px/g)]
    .map(([, k, l, w]) => ({ k, left: Number(l), right: Number(l) + Number(w) }));
  const track = Number(/rb-track" style="width:([\d.]+)px/.exec(html)[1]);
  const starts = placed.filter((b, i) => !i || b.k !== placed[i - 1].k).map((b) => b.left);
  if (starts.length === c.shape.length) {
    let at = 0;
    c.shape.forEach(([, len], i) => {
      const want = (at / drawn) * track;
      if (Math.abs(starts[i] - want) > TOLERANCE) {
        fail(`${c.id}: run ${i} starts at ${starts[i].toFixed(1)}px, but drawn message ` +
             `${at} of ${drawn} is ${want.toFixed(1)}px along a ${track.toFixed(1)}px track`);
      }
      at += len;
    });
  }

  // 4. The track. The bands fill the width they were given — both ends, because an axis
  //    counting the wrong messages fills a fraction of it and still looks like a ribbon.
  if (placed.length) {
    const reach = Math.max(...placed.map((b) => b.right));
    if (reach > track + TOLERANCE || reach < track - TOLERANCE) {
      fail(`${c.id}: bands reach ${reach.toFixed(1)}px of a ${track.toFixed(1)}px track`);
    }
  }

  drawnTotal += drawn;
  bandTotal += drawnBands.length;
}

/* ------------------------------------------------------------ fixtures ---- */

// No export, no shape: the invented conversations in data.js are what gallery.html and
// directions.html render, and they still have to draw. That fallback is the last copy of
// the run-length rule in the prototype and this is what keeps it honest.
const fixtures = load(app);
const bandless = fixtures.conversations.filter((c) => !bandsIn(fixtures.ctx.CS_UI.ribbon(c, true)).length);
if (bandless.length) fail(`${bandless.length} fixtures draw no bands without an export`);
const shaped = fixtures.conversations.filter((c) => c.shape);
if (shaped.length) fail(`${shaped.length} fixtures claim a shape they cannot have`);

/* ---------------------------------------------------------------- said ---- */

const n = real.conversations.length;
console.log(`${n} conversations · ${drawnTotal.toLocaleString()} drawn messages · ` +
            `${bandTotal.toLocaleString()} bands drawn from ` +
            `${real.conversations.reduce((a, c) => a + (c.shape || []).length, 0).toLocaleString()} runs`);
console.log(`${fixtures.conversations.length} fixtures draw without an export`);

if (failures.length) {
  console.error(`\n${failures.length} failures:`);
  for (const f of failures.slice(0, 20)) console.error(`  ${f}`);
  process.exit(1);
}
console.log('the ribbon draws the shape cs sent');
