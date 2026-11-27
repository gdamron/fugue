# Previewing notes

`FugueEngine.previewNotes` lists the notes a document will play over a range
of bars, without playing it. A page shows a piano roll or a score of a piece
before anyone presses play, or checks an edit before reloading it. The
browser engine (the wasm-bindgen web build) has it. In Rust it is
[`NotePreview`](../src/invention/preview.rs). The `preview_notes` tool, locally
and in the player, answers with it, at `PREVIEW_SAMPLE_RATE` (48 kHz), so both
list the same notes.

```js
const answer = JSON.parse(await engine.previewNotes(json, JSON.stringify({ bars: 8 })));
```

`json` is a complete invention document, as `loadInvention` takes it. The
call returns a `Promise` that resolves to the answer as JSON, or rejects with
a string.

## What it does

The engine builds a private copy of the document at its own sample rate.
The copy is built under the request's seed and started without script hosts
or agent workers. The engine then runs the copy offline, reading the notes
from the same note taps the live `get_notes` drains. The engine's own piece
is never touched: its position, its controls and its output stay as they
were. The copy is dropped before the promise settles.

The work runs on the calling thread in slices of about 8 ms. Between slices
it yields to the event loop with `setTimeout(0)`, so a page stays responsive
and audio rendered between slices is unaffected. No Worker is needed; the
call also works inside one. Do not call it from an AudioWorklet, which has
no `setTimeout`.

## Request

| field | type | default | meaning |
|---|---|---|---|
| `bars` | integer, 1 to 64 | 4 | how many bars the range spans |
| `start_bar` | integer, 0 or more | 0 | first bar of the range, counted from 0 at the start of the piece |
| `module_id` | string | none | list only this top-level module's notes |
| `seed` | integer, 0 to 2^53-1 | 0 | the invention seed for random modules with no seed of their own |
| `cursor` | string | none | a previous answer's `cursor`, to get the next page; pass the same document and fields |

Bars are bars of the piece's main clock: the clock `main_clock` names, or
else its first clock. The meter is the one that clock starts with, 4 beats
when it reports none. Without a clock, a bar is 2 seconds. The range is
`[start_bar, start_bar + bars)`. A note is in the range when it starts there.

## Limits

These are refused before any audio is rendered:

- `bars` at most **64**: `bars is 65; the largest allowed is 64`.
- At most **300 seconds** of audio, `300 × sampleRate` frames, counted from
  the start of the piece to one bar past the range at the starting tempo:
  `the range needs 13671000 frames (310 seconds at 44100 Hz); the largest allowed is 13230000 frames (300 seconds)`.
- A document the engine cannot load is refused with exactly the string
  `loadInvention` throws for it.
- An unknown `module_id` is refused, naming the id.
- A malformed request, or a cursor that does not belong to the request, is
  refused, naming the field.

If the tempo rises during the run and the frame limit is reached anyway, the
run stops there and the answer says `limit_reached: true`.

## Answer

```json
{
  "notes": [
    { "module_id": "upper", "voice": "0", "pitch": 60, "velocity": 1.0,
      "clock": "clock", "start_beats": 0.5, "end_beats": 0.95,
      "start_seconds": 0.166667, "end_seconds": 0.316667 }
  ],
  "range": { "start_bar": 0, "bars": 8, "clock": "clock", "beats_per_bar": 4,
             "start_seconds": 0.0, "end_seconds": 10.666667 },
  "seed": 0,
  "chance": { "involved": false, "modules": [], "message": "..." },
  "not_run": [],
  "truncated": false,
  "cursor": null,
  "limit_reached": false,
  "limits": { "max_bars": 64, "max_frames": 13230000 }
}
```

- A note has the shape of a played note from `PollNotes` (`ListedNote` in
  [`rpc/notes.rs`](../src/rpc/notes.rs)), without its `seq` and `id`: the
  same fields in the same units.
- `pitch` is the MIDI note, rounded. `velocity` is 0 to 1, to four places.
- `voice` is `"0"` for a module's lone `gate` and `"N"` for `gate.N`.
- `start_beats` and `end_beats` count on the note's `clock` from the start
  of the piece. They are null without a clock. Seconds count from the start
  of the piece. Times are rounded to the millionth.
- Ends are null for a note still sounding one bar past the range.
- Notes are in order of `start_seconds`; notes that start together are
  ordered by `module_id`, then `pitch`, then voice.
- `chance` names every `chance`, `melody` and `noise` module, inside
  inline developments too (as `instance/inner`). When there are any, the
  notes are one possible playing, and a different seed may give different
  notes.
- `not_run` names every `code` and `agent` module, inside inline
  developments too.
  Their scripts and agents never start, and the rest of the piece plays as
  usual.

## Pages

One answer holds at most **256** notes and at most **48 KiB** of JSON. When
more remain, `truncated` is true and `cursor` is a string. Pass the cursor
back with the same document and fields to get the next notes, with none
missing and none repeated. The last page has `truncated: false` and
`cursor: null`. The engine keeps nothing between calls. Each page runs the
copy again, so asking for a page costs as much as asking for the first.
