#!/usr/bin/env python3
"""Emit visual directions' tokens as the Swift file the macOS app reads.

The values in `styles.css` and `directions.css` were solved once — half of them by
`palette.py` against measurements, the rest by hand over many passes — and they are
solved *in both themes*, which is the expensive half. Typing forty colours twice into
a Swift file would put a second copy of that work somewhere it can rot, and would put
it there via the one operation nobody checks: transcription. `palette.py --verify`
exists because solving a colour and pasting it into a stylesheet are two events and
only one of them was checked. This script removes the second event.

    python3 tokens.py terminal                          # to stdout
    python3 tokens.py terminal paper blueprint ink \\
        -o ../../apps/macos/Sources/CsTheme/Tokens.swift

SEVERAL DIRECTIONS, ONE FILE. Naming more than one emits them all, declares the list,
and makes the first one `shipped` — which is what lets a build carry several palettes
and pick between them at launch (`chat-search-me9.8.9`). The alternative was a file each
plus a hand-written file to bind them, and it was rejected for what it costs rather
than for how it looks: the binding is the one part that has to agree with the set, and
a hand-written binding is the place that agreement rots. Generating the list means a
direction that is compiled in is a direction the app offers and a direction
`--verify-theme` measures, with no second list anywhere to disagree.

The output is checked in, because the app must build from a checkout with no Python in
it. What this buys is that regenerating is a diff: swapping the whole palette is one
command against one file, and `git diff --stat` says so rather than a comment claiming
it.

Companion to `palette.py`, whose cascade this reads the stylesheets through, and to
`icons.py`. Pure stdlib, no dependencies.
"""

import argparse
import os
import re
import sys

import palette

HERE = os.path.dirname(os.path.abspath(__file__))

# --------------------------------------------------------------- the map ----

# What the Swift side calls a token is derived from the CSS name rather than listed,
# so the two cannot fall out of step silently: a name only this file knows how to
# spell would fail to compile against the enum in Theme.swift, which is the check.
COLOURS = [
    '--bg', '--panel', '--panel-2', '--panel-3',
    '--ink', '--ink-2', '--ink-3',
    '--rule', '--rule-2',
    '--sel', '--sel-bg', '--sel-ink',
    '--hit', '--hit-bg',
    '--err', '--ok',
    '--k-user', '--k-agent', '--k-reason', '--k-tool',
    '--act-look', '--act-run', '--act-change',
    '--k-tool-ink', '--map-bg',
    '--src-claude-code', '--src-codex', '--src-chatgpt', '--src-claude-ai',
    '--src-gemini',
]
SIZES = ['--fs-head', '--fs-body', '--fs-sub', '--fs-meta', '--fs-micro']
FACES = ['--ui', '--mono', '--serif']
# The Swift names for these read as words rather than as abbreviations; everything
# else is mechanical. Radii keep their numbers because the CSS scale is ad hoc on
# purpose — `styles.css` says imposing an order on it would quietly make one
# direction's answer the default for all of them.
METRICS = {
    '--r-1': 'r1', '--r-2': 'r2', '--r-3': 'r3',
    '--r-4': 'r4', '--r-5': 'r5', '--r-6': 'r6',
    '--row-px': 'rowPaddingX', '--row-pt': 'rowPaddingTop',
    '--row-pb': 'rowPaddingBottom', '--row-lead': 'rowLead', '--row-gap': 'rowGap',
    '--rib-h': 'ribbonHeight', '--rib-track': 'ribbonTrack', '--rib-top': 'ribbonTop',
}


def swift_name(css):
    """`--k-tool-ink` -> `kToolInk`. The same rule Theme.swift's enum was written by."""
    head, *rest = css.lstrip('-').split('-')
    return head + ''.join(w[:1].upper() + w[1:] for w in rest)


# ------------------------------------------------------------- the values ----


def hexit(value, token):
    """`#1b2123` -> `0x1b2123`. Anything else is a token this file cannot carry."""
    v = value.strip()
    if not re.fullmatch(r'#[0-9a-fA-F]{6}', v):
        sys.exit(f'{token}: expected a six-digit hex colour, got {v!r}')
    return '0x' + v[1:].lower()


def points(value, token):
    """CSS px -> points, one for one.

    The prototype renders at native metrics on purpose — a mock set in a display face
    stops telling the truth about density, which is the only reason it exists — so its
    numbers are already the numbers a native view wants. A unit other than px would
    not be, so it fails rather than guessing.
    """
    v = value.strip()
    if v in ('0', '0px'):
        return '0'
    m = re.fullmatch(r'(\d+(?:\.\d+)?)px', v)
    if not m:
        sys.exit(f'{token}: expected a px length, got {v!r}')
    return m.group(1)


# The generic family a stack falls back to is the part SwiftUI models, and on this
# platform it is also the part that is true: `-apple-system` resolves to the same face
# `Font.Design.default` asks for, and `ui-monospace` to `.monospaced`. A direction that
# names a specific face — `paper` asks for Iowan Old Style — lands on `.serif` here,
# which is the right generic and not the named face; Theme.swift says so where the
# `FaceToken` is declared.
DESIGNS = {'sans-serif': '.default', 'serif': '.serif', 'monospace': '.monospaced'}


def design(stack, token):
    last = stack.strip().rstrip(';').split(',')[-1].strip().strip('"\'')
    if last not in DESIGNS:
        sys.exit(
            f'{token}: the stack {stack!r} ends in {last!r}, which is not one of the '
            f'generic families SwiftUI models ({", ".join(DESIGNS)}). Give the stack a '
            f'generic fallback, or widen FaceToken to carry a family name.')
    return DESIGNS[last]


def collect(direction, theme):
    """Every token this file carries, for one direction and theme."""
    css = palette.resolve(direction, theme)
    missing = [t for t in COLOURS + SIZES + FACES + list(METRICS) if t not in css]
    if missing:
        sys.exit(f'{direction}/{theme}: no value in force for {", ".join(missing)}')
    return css


# -------------------------------------------------------------- the emit ----


def declaration(direction, w):
    """One direction, as the `static let` the app reads it through."""
    dark, light = collect(direction, 'dark'), collect(direction, 'light')

    # Type and geometry are theme-independent in both stylesheets — a direction sets
    # them on `.dir-X`, not on `.dir-X.theme-dark` — and the Swift shape assumes that,
    # holding one scale against two palettes. If a direction ever splits them the
    # assumption has to be revisited rather than silently resolved to the dark side.
    split = [t for t in SIZES + FACES + list(METRICS) if dark[t] != light[t]]
    if split:
        sys.exit(
            f'{direction}: {", ".join(split)} differs between the two themes, which '
            f'Theme holds one of. Either the direction is wrong or TypeScale and '
            f'Geometry have to become per-theme.')

    w(f'    public static let {swift_name(direction)} = Theme(\n')
    w(f'        name: "{direction}",\n')
    for label, css in (('dark', dark), ('light', light)):
        w(f'        {label}: Palette([\n')
        for token in COLOURS:
            w(f'            .{swift_name(token)}: RGB({hexit(css[token], token)}),'
              f'  // {token}\n')
        w('        ]),\n')
    w('        type: TypeScale(\n')
    w('            sizes: [\n')
    for token in SIZES:
        name = swift_name(token).removeprefix('fs')
        w(f'                .{name[:1].lower() + name[1:]}: {points(dark[token], token)},'
          f'  // {token}\n')
    w('            ],\n')
    w('            faces: [\n')
    for token in FACES:
        w(f'                .{swift_name(token)}: {design(dark[token], token)},'
          f'  // {token}\n')
    w('            ]),\n')
    w('        geometry: Geometry([\n')
    for token, name in METRICS.items():
        w(f'            .{name}: {points(dark[token], token)},  // {token}\n')
    w('        ]))\n')


# Names the generated file declares itself, and therefore names no direction may take.
RESERVED = {'directions', 'shipped'}


def emit(directions, out):
    properties = {}
    for direction in directions:
        prop = swift_name(direction)
        if prop in RESERVED:
            sys.exit(f'{direction}: this file declares `{prop}` itself, so a direction '
                     f'cannot be called that. Rename it in palette.py.')
        if prop in properties:
            sys.exit(f'{direction} and {properties[prop]} both spell `{prop}` in Swift.')
        properties[prop] = direction

    w = out.write
    w(f'''// Generated by `python3 poc/ui/tokens.py {" ".join(directions)}`. Do not edit by hand.
//
// The values are `poc/ui/styles.css` and `poc/ui/directions.css` read through the same
// cascade `poc/ui/palette.py --verify` reads them through, so the app and the prototype
// cannot drift: there is one authored copy of these palettes and it is the one the mockup
// renders. This file is a build product that happens to be checked in, because the app
// has to build from a checkout with no Python in it.
//
// It is provenance and not a dependency. Nothing in `apps/` reads `poc/` at build or at
// run time, and regenerating is a thing a person does on purpose.
//
// `swift run -c release chat-search --verify-theme` re-measures every direction below
// against the measurements that fence it, for the reason `palette.py --verify` exists:
// solving a colour and writing it down are two events, and only one of them was ever
// checked. Every direction here is one the app offers, so every one of them is measured.

import SwiftUI

extension Theme {{
''')
    for i, direction in enumerate(directions):
        if i:
            w('\n')
        declaration(direction, w)

    # Both bindings are generated, and that is the point of the seam: the set of
    # directions and the one in force appear in this file and nowhere else, so adding a
    # direction or changing the default is this file and no view. A hand-written list
    # would be a second place that has to agree with the declarations above, and the
    # first time it did not the app would offer a theme it does not carry.
    w('''
    /// Every direction compiled into this build, in the order they were generated.
    ///
    /// Generated rather than written, so this list cannot disagree with what is above it:
    /// a direction that is compiled in is one the app offers and one `--verify-theme`
    /// measures. That equivalence is what makes "everything the picker offers is fenced"
    /// enforceable rather than a habit (docs/DECISIONS.md ADR 25).
''')
    w('    public static let directions: [Theme] = ['
      + ', '.join(swift_name(d) for d in directions) + ']\n')
    w('''
    /// What the app draws when nobody has chosen: the first direction named on the command
    /// line that generated this file.
    ///
    /// The environment's default, so no view names a direction and no view names a value.
    /// Changing which direction that is stays a regeneration of this one file — the claim
    /// the seam makes, and `git diff --stat` is the evidence.
''')
    w(f'    public static let shipped = {swift_name(directions[0])}\n')
    w('}\n')


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('directions', nargs='+', choices=sorted(palette.DIRECTIONS),
                    metavar='direction',
                    help='one or more of: ' + ', '.join(sorted(palette.DIRECTIONS))
                         + '. The first is what the app draws by default.')
    ap.add_argument('-o', '--output', help='write here instead of stdout')
    args = ap.parse_args()

    if args.output:
        with open(args.output, 'w') as f:
            emit(args.directions, f)
    else:
        emit(args.directions, sys.stdout)


if __name__ == '__main__':
    sys.exit(main())
