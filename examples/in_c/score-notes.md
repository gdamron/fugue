# In C Score Notes

This asset transcribes the 53 melodic cells from Terry Riley's *In C* (1964)
for Fugue's `cell_sequencer`.

## Source and Credit

Composer: Terry Riley.

Primary source to audit against: the *In C* score PDF attached to the Linear
"Generative In C" project.

This data pass was seeded from an existing machine-readable realization of the
53 cells. Cells 1 and 2 have been corrected against the project PDF for the
32nd-note grid and visible grace notes. Before treating the whole asset as
publication-quality score data, audit the remaining cells against the project
PDF.

## Encoding Choices

- `base_note_hint` is `60` (C4).
- `cells` are arrays of `cell_sequencer` steps. Each step uses `{ "note": n }`
  for a pitch offset from C4, `{ "held": true }` to continue the previous
  active note without retriggering, or `{ "note": null }` for a rest.
- `rhythm_grid` is `32nd_note`. Durations are expanded into repeated grid
  steps because the current step shape has no duration field.
- Sustained notes are represented as one note step followed by held steps on the
  32nd-note grid. Held steps keep the sequencer gate high and retain the previous
  frequency; sustain-capable voices can sustain, while percussive voices can
  still decay internally.
- Repeated note steps indicate rearticulation. Held steps indicate continuation.
- Grace notes are encoded as one 32nd-note grid step where they appear in the
  transcription. In cells 1 and 2, the leading grace C is encoded before the
  principal note.

## Playback Notes

Each performer should use its own `cell_sequencer` instance. The score data is
shared as configuration data only; sequencer playback state must not be shared
between performers.

When auditioning a cell directly, set `cell_sequencer.steps` to that cell's
length. Cell 35 requires 256 steps on the 32nd-note grid, so
`cell_sequencer` supports cells up to 256 steps.
