#!/usr/bin/env python3
"""Solve the luminance-critical tokens for a visual direction, and print them as CSS.

Most of a direction's palette is a choice. Eight of its tokens are not, because two
measurements in DESIGN-BRIEF.md fence them:

  1. The four message kinds must sit on an even luminance ramp against the ribbon
     track, because hue is the channel that degrades fastest at the ~2px the bands
     are drawn at. The incumbent's ramp is 2.2 / 4.0 / 7.2 / 13.0 - an even ~1.8x
     per step, the widest even spacing four levels fit into the range.
  2. The quiet text tier carries dates, counts and section labels rather than
     decoration, and must clear 4.5:1 at 9-11px. The incumbent sits at 4.6.

So a direction picks a *hue* for each of those and this script solves the lightness.
Every direction therefore reads identically at 2px however different it looks at 200,
which is the point: the ramp is the finding, the colours are not.

    python3 palette.py             # every direction, as CSS blocks plus a check table
    python3 palette.py paper       # one direction
    python3 palette.py --verify    # re-measure what directions.css actually says

Exits non-zero if any direction misses either measurement, so it can be run as a check
and not only as a generator. Companion to icons.py: pure stdlib, no dependencies.
"""

import sys

# ---------------------------------------------------------------- colour ----


def srgb_to_linear(c):
    c /= 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def luminance(rgb):
    r, g, b = (srgb_to_linear(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b):
    la, lb = luminance(a), luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def hsl_to_rgb(h, s, l):
    """h in degrees, s and l in 0..1. Returns three ints in 0..255."""
    c = (1 - abs(2 * l - 1)) * s
    hp = (h % 360) / 60.0
    x = c * (1 - abs(hp % 2 - 1))
    r, g, b = [(c, x, 0), (x, c, 0), (0, c, x),
               (0, x, c), (x, 0, c), (c, 0, x)][int(hp) % 6]
    m = l - c / 2
    return tuple(round((v + m) * 255) for v in (r, g, b))


def parse_hex(s):
    s = s.lstrip('#')
    return tuple(int(s[i:i + 2], 16) for i in (0, 2, 4))


def to_hex(rgb):
    return '#%02x%02x%02x' % rgb


def pivot_lightness(hue, sat, ground_luminance):
    """The lightness at which (hue, sat) matches the ground's luminance exactly.

    Luminance rises monotonically with HSL lightness, so this bisects cleanly - and it
    is the floor of the ramp: contrast against the ground is 1:1 here and rises in
    both directions away from it.
    """
    lo, hi = 0.0, 1.0
    for _ in range(60):
        mid = (lo + hi) / 2
        if luminance(hsl_to_rgb(hue, sat, mid)) < ground_luminance:
            lo = mid
        else:
            hi = mid
    return (lo + hi) / 2


def solve(hue, sat, target, ground):
    """Lightest/darkest colour at (hue, sat) that hits `target` contrast on `ground`.

    Contrast against a ground is V-shaped in lightness, bottoming out at 1:1 where the
    two luminances meet, so a bisect has to be told which arm of the V it is on before
    it starts. The search runs away from the ground: on a dark track a kind gets
    brighter, on a light one it gets darker, and "an even ramp" means the same ratios
    either way. That is why the light theme is not the dark theme inverted - it is the
    same solve with the inequality flipped.
    """
    g = parse_hex(ground) if isinstance(ground, str) else ground
    up = luminance(g) < 0.5
    # A target the ramp cannot reach on this ground has to fail loudly. Silently
    # returning the nearest wall is how two kinds end up 1.12:1 apart again.
    reach = contrast(hsl_to_rgb(hue, sat, 1.0 if up else 0.0), g)
    if reach < target:
        raise ValueError(
            f'unreachable: hue {hue} at saturation {sat} on {to_hex(g)} tops out at '
            f'{reach:.2f}:1, below the {target}:1 asked for. Darken the track (dark '
            f'theme) or lighten it (light theme), or drop the saturation.')
    pivot = pivot_lightness(hue, sat, luminance(g))
    lo, hi = (pivot, 1.0) if up else (0.0, pivot)
    for _ in range(60):
        mid = (lo + hi) / 2
        below = contrast(hsl_to_rgb(hue, sat, mid), g) < target
        if up:
            lo, hi = (mid, hi) if below else (lo, mid)
        else:
            lo, hi = (lo, mid) if below else (mid, hi)
    # The bisect converges on the ideal lightness, but the answer has to survive
    # rounding to 8 bits, and rounding is what turns "4.60:1" into "4.58:1" in a
    # published table. Step away from the ground until the *rounded* colour clears the
    # target, so the number in the CSS is the number a browser will measure.
    best = (lo + hi) / 2
    step = (1 if up else -1) / 2040.0
    while 0.0 <= best <= 1.0 and contrast(hsl_to_rgb(hue, sat, best), g) < target:
        best += step
    return to_hex(hsl_to_rgb(hue, sat, best))


# ------------------------------------------------------------ directions ----

# Ratios held constant across every direction, so the four kinds stay as far apart in
# the one channel that survives 2px no matter what a direction does to hue. Changing a
# number here re-opens a measurement; changing a hue below does not.
RAMP = {'tool': 2.20, 'reason': 4.00, 'user': 7.20, 'agent': 13.00}
# Sub-shades inside the tool band, by what the call was for. Deliberately narrow -
# 1.29x and 1.41x - because the primary read (tool vs prose vs reasoning) has to stay
# on the main ramp. `run` is the tool colour itself, so an ordinary agentic run looks
# exactly as it did.
ACTS = {'look': 1.70, 'run': 2.20, 'change': 3.10}
QUIET = 4.60  # the quiet tier, against whichever ground is harder for it


def quiet_ground(spec):
    """Of --bg and --panel, the one the quiet tier has the worse time on.

    The tier's 2.90-to-4.60 fix measured it against --bg, and in a light theme --bg is
    the *easier* ground: --panel is darker, so dark text on it has less to work with.
    But --panel is where most of that text actually lives — `.pv-meta`, and every tool
    and reasoning line in the transcript, all sit on the drawer. Solving against --bg
    alone left the light theme at 4.23:1 on the ground that matters, under the 4.5 AA
    floor for text this size, which is the same failure the fix was for.

    Dark themes are the other way round and pass either way; taking the harder of the
    two is the rule that is right in both.
    """
    bg, panel = parse_hex(spec['bg']), parse_hex(spec['panel'])
    dark_theme = luminance(bg) < 0.5
    # Light text: the harder ground is the lighter one. Dark text: the darker one.
    return to_hex(max(bg, panel, key=luminance) if dark_theme
                  else min(bg, panel, key=luminance))

# (hue, saturation) per solved token. Lightness is never written down here - that is
# the whole point. `bg` and `map` are the grounds the solve happens against and are the
# direction's own choice; they are repeated in directions.css, which is what the
# browser actually reads. If the two ever drift apart these numbers stop describing the
# page, which is why directions.html re-measures the rendered result rather than
# trusting this file.
DIRECTIONS = {
    'terminal': {
        'note': 'the incumbent, named so it can be compared rather than assumed',
        'dark': {'bg': '#1b2123', 'panel': '#161c1e',
                 'map': '#10191a',
                 'kinds': {'tool': (194, 0.15), 'reason': (274, 0.42),
                           'user': (178, 0.62), 'agent': (168, 0.12)},
                 'acts': {'look': (194, 0.13), 'run': (194, 0.15),
                          'change': (168, 0.21)},
                 'quiet': (190, 0.09)},
        'light': {'bg': '#fdfdfc', 'panel': '#f1f4f4',
                  'map': '#eaeeee',
                  'kinds': {'tool': (194, 0.15), 'reason': (274, 0.42),
                            'user': (178, 0.62), 'agent': (168, 0.12)},
                  'acts': {'look': (194, 0.13), 'run': (194, 0.15),
                           'change': (168, 0.21)},
                  'quiet': (190, 0.09)},
    },
    'paper': {
        'note': 'warm editorial - ink on stock, square corners, rules mostly withdrawn',
        'dark': {'bg': '#1c1917', 'panel': '#15120f',
                 'map': '#100e0c',
                 'kinds': {'tool': (32, 0.10), 'reason': (286, 0.34),
                           'user': (210, 0.44), 'agent': (38, 0.16)},
                 'acts': {'look': (32, 0.08), 'run': (32, 0.10),
                          'change': (96, 0.20)},
                 'quiet': (36, 0.12)},
        'light': {'bg': '#f6f1e6', 'panel': '#efe9db',
                  'map': '#e8e1d1',
                  'kinds': {'tool': (32, 0.14), 'reason': (286, 0.36),
                            'user': (210, 0.46), 'agent': (38, 0.18)},
                  'acts': {'look': (32, 0.10), 'run': (32, 0.14),
                           'change': (96, 0.24)},
                  'quiet': (36, 0.16)},
    },
    'blueprint': {
        'note': 'engineering drawing - mono throughout, square, the grid made visible',
        'dark': {'bg': '#0f1720', 'panel': '#0b131b',
                 'map': '#070d14',
                 'kinds': {'tool': (210, 0.20), 'reason': (266, 0.44),
                           'user': (196, 0.66), 'agent': (208, 0.14)},
                 'acts': {'look': (210, 0.16), 'run': (210, 0.20),
                          'change': (168, 0.30)},
                 'quiet': (208, 0.16)},
        'light': {'bg': '#f4f7fa', 'panel': '#e9eef5',
                  'map': '#e2e9f1',
                  'kinds': {'tool': (210, 0.22), 'reason': (266, 0.46),
                            'user': (196, 0.70), 'agent': (208, 0.16)},
                  'acts': {'look': (210, 0.18), 'run': (210, 0.22),
                           'change': (168, 0.34)},
                  'quiet': (208, 0.18)},
    },
    'ink': {
        'note': 'near-monochrome - the ramp carried on luminance almost alone',
        'dark': {'bg': '#1a1a1a', 'panel': '#141414',
                 'map': '#0e0e0e',
                 'kinds': {'tool': (210, 0.05), 'reason': (280, 0.13),
                           'user': (186, 0.16), 'agent': (0, 0.00)},
                 'acts': {'look': (210, 0.04), 'run': (210, 0.05),
                          'change': (162, 0.10)},
                 'quiet': (0, 0.00)},
        'light': {'bg': '#fbfaf9', 'panel': '#f2f1ef',
                  'map': '#e9e8e6',
                  'kinds': {'tool': (210, 0.06), 'reason': (280, 0.15),
                            'user': (186, 0.18), 'agent': (0, 0.00)},
                  'acts': {'look': (210, 0.05), 'run': (210, 0.06),
                           'change': (162, 0.12)},
                  'quiet': (0, 0.00)},
    },
}


def solved(spec):
    out = {}
    for k, target in RAMP.items():
        out['--k-' + k] = solve(*spec['kinds'][k], target, spec['map'])
    for k, target in ACTS.items():
        out['--act-' + k] = solve(*spec['acts'][k], target, spec['map'])
    out['--ink-3'] = solve(*spec['quiet'], QUIET, quiet_ground(spec))
    return out


def report(names):
    for name in names:
        d = DIRECTIONS[name]
        print(f'/* {name} - {d["note"]} */')
        for theme in ('dark', 'light'):
            print(f'.dir-{name}.theme-{theme} {{')
            for k, v in solved(d[theme]).items():
                print(f'  {k}: {v};')
            print('}')
        print()

    print('/* ' + '-' * 70)
    print('   check: every direction against the two fenced measurements')
    print()
    print(f'   {"direction":<11} {"theme":<6} ' +
          ' '.join(f'{k:>8}' for k in ('tool', 'reason', 'user', 'agent')) +
          '   ' + ' '.join(f'{s:>4}' for s in ('t>r', 'r>u', 'u>a')) + '  quiet')
    ok = True
    for name in names:
        for theme in ('dark', 'light'):
            spec = DIRECTIONS[name][theme]
            t = solved(spec)
            ratios = [contrast(parse_hex(t['--k-' + k]), parse_hex(spec['map']))
                      for k in ('tool', 'reason', 'user', 'agent')]
            steps = [b / a for a, b in zip(ratios, ratios[1:])]
            quiet = contrast(parse_hex(t['--ink-3']),
                             parse_hex(quiet_ground(spec)))
            ok &= quiet >= QUIET and all(abs(s - 1.8) < 0.06 for s in steps)
            print(f'   {name:<11} {theme:<6} ' +
                  ' '.join(f'{r:>6.2f}:1' for r in ratios) +
                  '   ' + ' '.join(f'{s:.2f}' for s in steps) +
                  f'  {quiet:.2f}:1')
    print()
    print('   The three numbers before ink-3 are the steps between adjacent kinds.')
    print('   The finding is that they stay even and near 1.8x, not that any one')
    print('   colour is right. Averaging them would hide exactly the failure this')
    print('   is here to catch, so all three are printed.')
    print('   ' + ('all directions hold both measurements.' if ok else
                   'FAILED - a direction misses a fenced measurement.'))
    print('   ' + '-' * 70 + ' */')
    return 0 if ok else 1



# ---------------------------------------------------------------- verify ----
# Solving a colour and pasting it into a stylesheet are two events, and only one of
# them is checked by the code above. This reads the CSS back — the same text the
# browser reads — and re-measures every direction from it, so a transposed digit
# surfaces here rather than in a screenshot six weeks later.

import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
BLOCK = re.compile(r'([^{}]+)\{([^{}]*)\}')
DECL = re.compile(r'(--[a-z0-9-]+)\s*:\s*([^;]+);')


def read_blocks(path):
    """Selector -> {token: value}, comments stripped. Enough CSS to read tokens."""
    text = re.sub(r'/\*.*?\*/', '', open(path).read(), flags=re.S)
    out = []
    for sel, body in BLOCK.findall(text):
        decls = {k: v.strip() for k, v in DECL.findall(body)}
        if decls:
            for one in sel.split(','):
                out.append((one.strip(), decls))
    return out


def resolve(direction, theme):
    """Every token in force for one direction and theme, in cascade order.

    Not a CSS engine — it only has to be right about single-class selectors carrying
    custom properties, which is all either file uses.

    The tiers below are specificity, and they are tiers rather than document order
    because the two disagree. `:root` appears twice in styles.css — once for the dark
    colours at the top and once for the type scale and the geometry near the middle —
    with `:root.light` sitting between them. Walking the file in order would let the
    second `:root` overwrite the light theme, and would drop the type scale and the
    geometry from the light theme entirely, since the light tier never included plain
    `:root`. Nothing noticed while only colours were read back; `tokens.py` reads the
    whole surface layer and would have emitted a light theme with no sizes in it.
    """
    tiers = [
        [':root'],
        [':root.light', '.theme-light'] if theme == 'light' else ['.theme-dark'],
        [f'.dir-{direction}'],
        [f'.dir-{direction}.theme-{theme}'],
    ]
    blocks = []
    for path in ('styles.css', 'directions.css'):
        blocks += read_blocks(os.path.join(HERE, path))
    tokens = {}
    for tier in tiers:
        for sel, decls in blocks:
            if sel in tier:
                tokens.update(decls)
    return tokens


def check(name, fg, bg, floor, tokens, notes):
    """One contrast reading against a floor. Returns (ok, line)."""
    try:
        ratio = contrast(parse_hex(tokens[fg]), parse_hex(tokens[bg]))
    except KeyError as e:
        notes.append(f'      missing token {e}')
        return False, f'      {name:<34} —'
    ok = ratio >= floor
    return ok, (f'      {name:<34} {ratio:>6.2f}:1  '
                f'{"ok" if ok else f"UNDER {floor}"}')


def verify(names):
    """Re-measure every direction from the stylesheets, not from DIRECTIONS."""
    ok = True
    for name in names:
        print(f'{name}')
        for theme in ('dark', 'light'):
            t = resolve(name, theme)
            notes = []
            print(f'  {theme}')

            ratios = [contrast(parse_hex(t['--k-' + k]), parse_hex(t['--map-bg']))
                      for k in ('tool', 'reason', 'user', 'agent')]
            steps = [b / a for a, b in zip(ratios, ratios[1:])]
            even = all(abs(s - 1.8) < 0.06 for s in steps)
            ok &= even
            print('      kind ramp on the track           ' +
                  ' '.join(f'{r:.2f}' for r in ratios) +
                  '   steps ' + ' '.join(f'{s:.2f}x' for s in steps) +
                  ('  ok' if even else '  UNEVEN'))

            acts = [contrast(parse_hex(t['--act-' + k]), parse_hex(t['--map-bg']))
                    for k in ('look', 'run', 'change')]
            inside = acts[0] < acts[1] < acts[2] < ratios[1]
            ok &= inside
            print('      act shades inside the tool band  ' +
                  ' '.join(f'{r:.2f}' for r in acts) +
                  ('       ordered, under reasoning  ok' if inside
                   else '       OUT OF BAND'))

            # 4.5:1 is the AA floor for text at these sizes. The quiet tier is the one
            # the brief names, but every tier that carries information is checked:
            # a direction that fixes --ink-3 and breaks --k-tool-ink has not held the
            # finding, it has held the one token the finding happened to mention.
            for label, fg, bg, floor in [
                ('body text on its ground', '--ink', '--bg', 4.5),
                ('second tier on its ground', '--ink-2', '--bg', 4.5),
                ('quiet tier on the page', '--ink-3', '--bg', 4.5),
                ('quiet tier on the drawer', '--ink-3', '--panel', 4.5),
                ('tool names in the drawer', '--k-tool-ink', '--panel', 4.5),
                ('a selected row title', '--sel', '--sel-bg', 4.5),
                ('text inside a match', '--ink', '--hit-bg', 4.5),
            ]:
                good, line = check(label, fg, bg, floor, t, notes)
                ok &= good
                print(line)
            for n in notes:
                print(n)
        print()
    print('all directions hold.' if ok else 'FAILED — see the lines marked above.')
    return 0 if ok else 1


if __name__ == '__main__':
    args = sys.argv[1:]
    checking = '--verify' in args
    wanted = [a for a in args if a != '--verify'] or list(DIRECTIONS)
    unknown = [w for w in wanted if w not in DIRECTIONS]
    if unknown:
        sys.exit(f'unknown direction(s): {", ".join(unknown)}; '
                 f'have {", ".join(DIRECTIONS)}')
    sys.exit(verify(wanted) if checking else report(wanted))
