#!/usr/bin/env python3
"""Emit one visual direction's tokens as the Swift file the macOS app reads.

The values in `styles.css` and `directions.css` were solved once — half of them by
`palette.py` against measurements, the rest by hand over many passes — and they are
solved *in both themes*, which is the expensive half. Typing forty colours twice into
a Swift file would put a second copy of that work somewhere it can rot, and would put
it there via the one operation nobody checks: transcription. `palette.py --verify`
exists because solving a colour and pasting it into a stylesheet are two events and
only one of them was checked. This script removes the second event.

    python3 tokens.py terminal                 # to stdout
    python3 tokens.py terminal -o ../../apps/macos/Sources/CsTheme/Tokens.swift

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


def emit(direction, out):
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

    w = out.write
    w(f'''// Generated by `python3 poc/ui/tokens.py {direction}`. Do not edit by hand.
//
// The values are `poc/ui/styles.css` and `poc/ui/directions.css` read through the same
// cascade `poc/ui/palette.py --verify` reads them through, so the app and the prototype
// cannot drift: there is one authored copy of this palette and it is the one the mockup
// renders. This file is a build product that happens to be checked in, because the app
// has to build from a checkout with no Python in it.
//
// It is provenance and not a dependency. Nothing in `apps/` reads `poc/` at build or at
// run time, and regenerating is a thing a person does on purpose.
//
// `swift run -c release chat-search --verify-theme` re-measures what is below against
// the measurements that fence it, for the reason `palette.py --verify` exists: solving a
// colour and writing it down are two events, and only one of them was ever checked.

import SwiftUI

extension Theme {{
''')
    w(f'    public static let {direction} = Theme(\n')
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
    # The binding is generated too, and that is the whole point of the seam: the name of
    # the direction in force appears in this file and nowhere else, so pointing the app
    # at another one is this file and no view. If `Theme.terminal` were what the
    # environment defaulted to, regenerating for `paper` would stop compiling until
    # somebody edited a view, which is the lift this bead exists to prevent.
    w('''
    /// The direction this build draws.
    ///
    /// Regenerating this file for another direction moves every colour, size and radius in
    /// the app and touches nothing else — no view names a direction, and no view names a
    /// value. That is the claim; `git diff --stat` after a regeneration is the evidence.
''')
    w(f'    public static let shipped = {direction}\n')
    w('}\n')


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('direction', choices=sorted(palette.DIRECTIONS))
    ap.add_argument('-o', '--output', help='write here instead of stdout')
    args = ap.parse_args()

    if args.output:
        with open(args.output, 'w') as f:
            emit(args.direction, f)
    else:
        emit(args.direction, sys.stdout)


if __name__ == '__main__':
    sys.exit(main())
