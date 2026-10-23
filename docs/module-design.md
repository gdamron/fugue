# Module design

This document is for contributors adding or changing a module type, a
development, or a view of running audio, and for reviewers of those changes.
It covers three standing rules:

1. [Behaviour lives in modules with ports](#behaviour-lives-in-modules-with-ports),
   not in properties that change how the graph is read.
2. [Interfaces follow one naming convention](#interface-naming), enforced by
   tests.
3. [Views of running audio show what is heard](#views-show-what-is-heard).

## Behaviour lives in modules with ports

When a behaviour can be a module with inputs and outputs, make it one. Do not
add a property to a development or a document that changes how its
connections are interpreted.

Two features were first built the other way and reworked:

- **Sustain pedal.** It was a development-level input mode that broadcast and
  latched a pedal across the development. It is now the `sustain` module
  ([`src/modules/sustain/mod.rs`](../src/modules/sustain/mod.rs)): `gate` and
  `pedal` in, `gate` out. Wired between a note gate and an envelope, it holds
  the gate while the pedal is down. A struck voice rings out because its
  envelope's sustain level is 0, not because anything special happens in the
  graph.
- **Polyphony.** It was a property that replicated a voice into a hidden pool.
  It is now the `divisi` module
  ([`src/modules/divisi/mod.rs`](../src/modules/divisi/mod.rs)): one line of
  `frequency`, `gate` and `velocity` in, `frequency.N`, `gate.N` and
  `velocity.N` out, one set per voice. The voices are ordinary modules or
  developments, declared one by one and connected by ordinary connections.

Why modules win:

- **Routing stays visible.** Every signal path is a connection someone can
  read, inspect and edit. A property hides routing inside the interpreter.
- **Cases compose.** Two instruments with independent pedals in one
  development are two `sustain` modules. A development-wide pedal property
  cannot express that.
- **Modules combine with everything else.** `sustain` works with any module
  that takes a gate, not one particular envelope.

### Fan-out is explicit

A development fans one exposed name out to several targets by listing the name
more than once:

- Repeating an `inputs` name routes that input to every listed `to` port.
- Repeating a `controls` name makes one write reach every listed inner control
  ([`src/invention/development/declared_controls.rs`](../src/invention/development/declared_controls.rs)).
  One `decay` can reach every voice.

That replaces any "broadcast" mode: the document lists exactly what each name
reaches.

### Reviewing a new property

A config key that shapes a module's own processing is fine. `sample_instrument`
keeps its own voice pool, sized by `voice_count`, inside the module. Push back
on a key whose effect is to rewire, replicate or reinterpret other modules or
connections. Ask whether it could be a module, or a repeated name, instead.

## Interface naming

The names a module exposes (inputs, outputs, controls and config keys) follow
one convention. So do the names a development exposes, and the fields of
formats that feed modules, such as `fugue.score.v1`. An author who knows one
module can then guess the names of the next one.

### Where it is enforced

- [`src/interface_naming.rs`](../src/interface_naming.rs): `naming_problems`
  checks one name and says which rules it breaks.
- [`src/interface_naming/tests/guard.rs`](../src/interface_naming/tests/guard.rs)
  builds every built-in type and checks every name it exposes, plus the rules
  that need the whole interface (pairs, indices, config against controls).
- [`src/interface_naming/tests/developments.rs`](../src/interface_naming/tests/developments.rs)
  checks the developments in `examples/` and in `FUGUE_DOCUMENT_DIRS`.
- A wasm guest's port names are checked as it loads
  ([`src/plugins/wasm.rs`](../src/plugins/wasm.rs)). A problem is printed as a
  warning and the guest still loads. Guests are built outside this repository,
  so refusing one would break music that worked yesterday.

Built-in types have no exceptions today.
[`exceptions.rs`](../src/interface_naming/tests/exceptions.rs) is empty, its
length can never grow past its ceiling, and a row that excuses nothing fails
the guard. A broken rule is fixed by renaming, never by adding a row.

### Spelling

- Lower `snake_case`.
- An indexed family is `stem.N`, counting from 0 with no leading zeros and no
  gaps: `level.0`, `level.1`. This holds for ports as well as controls.
- No digits glued onto a word (`in1`). The only exceptions are rate
  multipliers and divisors, `x2` and `d4`.

### Words

| Use                                          | Not                                    |
| -------------------------------------------- | -------------------------------------- |
| `audio`, or `audio_left` with `audio_right`  | `in`, `out`, `left`, `right` alone     |
| `level`, `<x>_level` for linear amplitude    | `gain`, `volume`                       |
| the musical quantity (`level`, `frequency`)  | `cv`                                   |
| the dimension (`waveform`, `filter_type`)    | `type` alone                           |
| seconds, with no suffix                      | `_ms`, `_seconds`                      |
| Hz, with no suffix                           | `_hz`                                  |
| a structured value under its plain name      | `_json`                                |
| the on state for a boolean (`loop`)          | `is_`, `no_`, `enable_`, `disable_`    |
| the moment for a trigger (`start`, `ended`)  | a `_gate` suffix                       |

`master`, `wet` and `dry` stay beside `level`. They are the words musicians
use for those mixes. `_left` and `_right` always come in pairs.

### Shared vocabulary

- **Modulation.** A modulation input is `<param>_mod`, and its amount is the
  control `<param>_mod_depth`. Each needs the other. Examples are
  `frequency_mod` on `oscillator`, `cutoff_mod` on `filter` and `rate_mod` on
  `lfo`.
- **Clocking.** The input that advances a step is `clock` (`step_sequencer`,
  `cell_sequencer`, `control_scheduler`). The `clock` module's outputs are
  `beat`, `beat_d4`, `beat_d2`, `beat_x2` and `beat_x4`. The pattern extends
  to new rates without new words.
- **Counts and arrays.** A plural names an array (`cells`, `samples`,
  `zones`). A count is `<noun>_count` (`step_count`, `voice_count`,
  `channel_count`). A config array that sets `level.N` is `levels`, never
  `channel_levels`: the `mixer` takes `levels` and `pans`.
- **Files.** A file a module reads is an `asset`.
- **Sequenced material.** A `cell_sequencer` plays `cells`: `cell_count`,
  `start_cell`, `select_cell`, `next_cell`, `previous_cell`. `start_cell` is
  config read once at build. The `cell` output and control report the cell
  playing now and cannot be written.

### Config and controls

- A config key that sets a control's starting value uses the control's key.
  It never respells it (`<type>_<key>`, `<key>_json`, `<key>_enabled`).
- Config is closed: an undeclared key is refused with an error naming the type
  and the key. The `code` module's free-form script parameters go under one
  `params` object, so its config can stay closed.
- Every key in a module's audio-thread control table is also a declared
  control. There is one table, not two that can drift apart.
- Telemetry is read-only. Examples are the `agent` module's `status`,
  `last_*` and `history`, and `cell_sequencer`'s `cell`. Writing one is
  refused.

### Developments and formats

A development names each exposed `inputs`, `outputs` and `controls` entry with
`name`. The old `key` field on controls no longer parses. Exposed names follow
the same rules, and use the musical gesture (`brightness`) rather than the
inner module's key (`cutoff`).

Formats that feed modules use the modules' names. A `fugue.score.v1` step has
`velocity`, `gate_length` and `root_note`, as the sequencers do
([`src/invention/score.rs`](../src/invention/score.rs)). A rename changes the
format in place rather than keeping the old spelling as an alias. Closed
formats then refuse the old name, so an old file fails to load instead of
silently losing data.

### Not checked yet

The guard does not yet check these parts of the convention:

- an input that shares a control's name and overrides it (`filter` has
  `cutoff` as both);
- units and roles: unsuffixed seconds, `_beats` for values in beats, and verbs
  only for events;
- nested config fields such as `zones[].root_note`.

Follow them by hand until the typed control tables carry enough to check them.

## Views show what is heard

A view of running audio shows what the listener hears. Today that is the
spectrogram. A future scope, loudness or waveform view follows the same rule.
Each reads the signal after the output stage has processed it, not the mix
before it. Clipping then shows up in the view the way it sounds.

The audio thread mixes every sink into the master block, scaling by
`1/sqrt(n)` for `n` sinks, and hands the finished block to
`MasterObservers::observe`
([`src/invention/graph/process.rs`](../src/invention/graph/process.rs),
[`src/invention/graph/master.rs`](../src/invention/graph/master.rs)). The
observers then shape it as the output stage does:

- **The spectrum tap** (`SpectrumTap::observe_block`,
  [`src/spectrum/tap.rs`](../src/spectrum/tap.rs)) clamps each channel to ±1,
  as the DAC's `write_frame` does
  ([`src/modules/dac/driver.rs`](../src/modules/dac/driver.rs)). It then sums
  the channels and halves them, as the DAC does for a mono device. The
  harmonics of a clipped signal show in the spectrum, and the analysis can
  never overflow. A signal panned hard to one side reads 6 dB below its
  channel peak, and an out-of-phase pair cancels.
- **The master level meter** is the exception
  ([`src/invention/graph/master/levels.rs`](../src/invention/graph/master/levels.rs)).
  Its peak and RMS read the mix before the clamp, per channel, so an over
  shows as a value above 1.0. A meter that stopped at 1.0 could not warn
  anyone that they are clipping.

Two small differences from the device path are deliberate:

- The DAC folds to mono only for a mono device, and sends a stereo device
  both channels. The spectrogram is always a single mono view.
- The DAC clamps the positive side just below 1.0 so cpal's 24-bit
  conversions do not wrap. The tap clamps at 1.0.

### Adding a view

- Add it as an observer in `MasterObservers`, not as another field threaded
  through every place a graph is built.
- Read the clamped signal unless the view exists to show overs, as the meter
  does. Say which one in the view's documentation.
- Keep the audio-thread side free of allocation and locks. Hand samples
  across with atomics, as the tap and the meter do, and do the analysis off
  the audio thread.
- When no one is reading, do nothing beyond a single relaxed load. The
  spectrum tap does exactly that.
