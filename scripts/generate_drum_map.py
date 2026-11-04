#!/usr/bin/env python3
"""Generates the `drum_map` module's pattern nodes.

Writes src/modules/drum_map/nodes.rs from the patterns authored below.
Run from the repository root after changing a pattern:

    python3 scripts/generate_drum_map.py          # rewrite nodes.rs
    python3 scripts/generate_drum_map.py --check  # fail if nodes.rs is stale

Sources and licences
--------------------
Every pattern below was authored for Fugue by its contributors and is
released with the rest of the repository under the MIT licence. They are
written from common drumming vocabulary (a backbeat, four on the floor,
straight and swung hats, syncopated and displaced kicks), which is general
musical knowledge rather than anyone's expression. No pattern, table or
constant was copied or transcribed from another drum machine, sequencer or
module; in particular nothing comes from Mutable Instruments' Grids (GPL-3.0),
whose source and tables were not consulted. No third-party data is used.

The map
-------
Nine nodes on a 3 x 3 grid. `x` moves across groove families, `y` up in
energy:

    y = 1  driving    | four on the floor | 16th-note funk    | broken breakbeat
    y = .5 standard   | rock backbeat     | boom-bap / funk   | broken beat
    y = 0  sparse     | half-time         | half-time hip-hop | half-step dub
                        x = 0 straight      x = .5 syncopated   x = 1 broken

Each node gives each voice (kick, snare, hat) a weight per 32nd-note step of
one 4/4 bar: '.' is 0 (the node never plays it) and 1-9 rank the steps, so
9s sound at the lowest density and 1s only near the top. A 16-character
pattern is written in 16ths and expands to the even 32nd steps; ORNAMENTS
add the odd 32nd steps (flams, rolls) that sound only at resolution 32.
"""

import pathlib
import sys

VOICES = ("kick", "snare", "hat")

# Rows from y = 0 (sparse) to y = 1 (driving); columns from x = 0 to x = 1.
# Beats fall on 16th steps 0, 4, 8 and 12:    "1...2...3...4..."
NODES = [
    # y = 0: sparse
    [
        {  # straight half-time: snare on beat 3
            "kick": "9......4..3....2",
            "snare": "........9....2..",
            "hat": "7.3.6.3.7.3.6.3.",
        },
        {  # half-time hip-hop: a late kick pushes into beat 3
            "kick": "9......7..6....3",
            "snare": "........9.....3.",
            "hat": "6.4.6.4.6.4.6.4.",
        },
        {  # half-step dub: wide gaps, hats in dotted 8ths
            "kick": "9.........7.....",
            "snare": "........9..2....",
            "hat": "5..4..6..4..5.3.",
        },
    ],
    # y = 0.5: standard
    [
        {  # rock backbeat
            "kick": "9.....4.8.3....2",
            "snare": "....9.....1.9..2",
            "hat": "8.4.7.4.8.4.7.4.",
        },
        {  # boom-bap / funk: kicks on the "and"s, a ghosted snare
            "kick": "9.....6...8...4.",
            "snare": "....9..2....93..",
            "hat": "8.6.8.6.8.6.8.36",
        },
        {  # broken beat: displaced kick and snare
            "kick": "9..7......8..4..",
            "snare": "....9..3.....9.4",
            "hat": "7.5.75.57.5.75.5",
        },
    ],
    # y = 1: driving
    [
        {  # four on the floor with off-beat hats
            "kick": "9...9...9...9..3",
            "snare": "....9.......9.2.",
            "hat": "3393339333933393",
        },
        {  # 16th-note funk
            "kick": "9..6..7...9..5..",
            "snare": "....9..3..4.9..5",
            "hat": "9575957595759575",
        },
        {  # broken breakbeat
            "kick": "9.7.......8.3...",
            "snare": "....9..4.6..9.5.",
            "hat": "9686968696869686",
        },
    ],
]

# Odd 32nd steps, by (row, column, voice): {32nd step: weight}.
ORNAMENTS = {
    (1, 1, "hat"): {23: 3},  # a flick into beat 4
    (2, 1, "hat"): {29: 3, 31: 4},  # a roll into the next bar
    (2, 2, "snare"): {15: 3, 27: 2},  # ghosted drags
    (2, 2, "hat"): {13: 3, 29: 3, 31: 4},
}

STEPS = 32
OUTPUT = pathlib.Path("src/modules/drum_map/nodes.rs")


def weights(pattern):
    """A pattern string as 32 weights."""
    if len(pattern) == 16:
        pattern = "".join(cell + "." for cell in pattern)
    if len(pattern) != STEPS:
        raise ValueError(f"pattern {pattern!r} is not 16 or 32 steps")
    return [0 if cell == "." else int(cell) for cell in pattern]


def nodes():
    built = []
    for row, columns in enumerate(NODES):
        for column, node in enumerate(columns):
            voices = []
            for voice in VOICES:
                cells = weights(node[voice])
                for step, weight in ORNAMENTS.get((row, column, voice), {}).items():
                    if step % 2 == 0 or cells[step] != 0:
                        raise ValueError(f"ornament {row},{column},{voice} {step}")
                    cells[step] = weight
                voices.append(cells)
            built.append(voices)
    return built


def render():
    lines = [
        "//! The drum map's pattern nodes. Generated by",
        "//! `scripts/generate_drum_map.py` from the patterns authored there (see",
        "//! its header for their sources and licences): edit the script, not this",
        "//! file.",
        "",
        "/// Nodes in rows from `y` = 0 up, each row from `x` = 0 across; per node,",
        "/// kick, snare and hat weights (0 to 9) for each 32nd step of the bar.",
        "#[rustfmt::skip]",
        f"pub(super) const NODES: [[[u8; {STEPS}]; 3]; 9] = [",
    ]
    for voices in nodes():
        lines.append("    [")
        for cells in voices:
            lines.append("        [" + ", ".join(str(c) for c in cells) + "],")
        lines.append("    ],")
    lines.append("];")
    return "\n".join(lines) + "\n"


def main():
    text = render()
    if "--check" in sys.argv[1:]:
        if OUTPUT.read_text() != text:
            sys.exit(f"{OUTPUT} is stale: run python3 scripts/generate_drum_map.py")
        return
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(text)


if __name__ == "__main__":
    main()
