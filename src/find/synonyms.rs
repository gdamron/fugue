//! The one table of musical synonyms a search widens a query with.
//!
//! Each row pairs the words a musician might use with the words Fugue uses
//! for the same thing: module types, control keys, views and tools. A query
//! word in a row also searches for that row's Fugue words, at a lower
//! weight than an exact match, so "brighter" reaches a filter's cutoff and
//! "echo" reaches the delay. Add a row, or a word to one, when a musical
//! request misses what it means; keep Fugue's words to ones the index
//! holds.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::rank::singular;

/// Musician's words, and the Fugue words each one also searches for.
#[rustfmt::skip]
pub const SYNONYMS: &[(&[&str], &[&str])] = &[
    (&["bright", "brighter", "brightness", "dark", "darker", "dull", "duller", "muffled", "muffle", "tone", "harsh", "mellow", "warm", "warmer"], &["cutoff", "filter"]),
    (&["resonant", "squelch", "peak", "peaky"], &["resonance", "filter"]),
    (&["echo", "repeat", "slapback", "dub"], &["delay", "feedback"]),
    (&["space", "spacious", "room", "hall", "ambience", "ambient", "wash", "verb"], &["reverb", "room_size", "wet"]),
    (&["big", "bigger", "large", "larger", "small", "smaller"], &["size"]),
    (&["loud", "louder", "quiet", "quieter", "softer", "volume", "amplitude", "fade"], &["level", "gain", "mixer", "master"]),
    (&["balance", "blend", "pan", "panning"], &["mixer", "level", "pan"]),
    (&["tempo", "fast", "faster", "slow", "slower", "quicker", "speed", "beat", "bpm"], &["bpm", "clock"]),
    (&["distortion", "distort", "overdrive", "saturate", "saturation", "crunch", "grit", "gritty", "dirty", "fuzz"], &["drive"]),
    (&["pitch", "tune", "tuning", "detune", "higher", "lower", "transpose"], &["frequency", "oscillator"]),
    (&["tone", "voice", "synth", "waveform", "wave", "sine", "saw", "square"], &["oscillator", "waveform"]),
    (&["melody", "tune", "riff", "line", "phrase", "motif"], &["melody", "note", "sequencer"]),
    (&["sequence", "pattern", "loop", "step", "steps"], &["step_sequencer", "cell_sequencer", "pattern", "sequencer"]),
    (&["note", "notes"], &["note", "pattern", "sequencer"]),
    (&["drum", "drums", "kick", "snare", "hat", "hihat", "percussion", "groove"], &["drum_map", "sample_kit", "kit"]),
    (&["envelope", "pluck", "swell", "shape"], &["adsr", "envelope", "attack", "decay", "release"]),
    (&["wobble", "vibrato", "tremolo", "modulate", "modulation", "movement", "oscillate"], &["lfo", "rate"]),
    (&["sweep", "swoosh", "jet", "phasing"], &["phaser"]),
    (&["random", "randomness", "chance", "probability", "generative", "dice", "surprise"], &["chance", "melody", "seed"]),
    (&["sample", "recording", "wav", "file", "audio"], &["sample_player", "sample_kit", "sample_instrument", "sample_slicer"]),
    (&["preview", "listen", "hear", "audition", "render", "bounce"], &["audio", "render", "preview"]),
    (&["chord", "chords", "polyphony", "polyphonic", "voices", "harmony"], &["divisi", "voice_count"]),
    (&["texture", "grain", "grains", "cloud", "shimmer"], &["granular"]),
    (&["compress", "compressor", "squash", "clip", "clipping", "limit"], &["limiter", "drive"]),
    (&["hiss", "static", "wind"], &["noise"]),
    (&["sustain", "hold", "pedal"], &["sustain"]),
    (&["piece", "song", "composition", "track", "patch"], &["invention"]),
    (&["favorite", "bookmark", "keep", "pin", "pinned"], &["pin", "pinned"]),
    (&["knob", "slider", "fader", "parameter", "param", "setting"], &["control"]),
    (&["wire", "patch", "route", "routing", "cable"], &["connect", "connection"]),
    (&["map", "diagram", "overview", "layout"], &["graph"]),
    (&["undo", "change", "tweak", "fix"], &["edit"]),
    (&["status", "playing", "running", "state"], &["status"]),
    (&["meter", "meters", "peak", "level"], &["meter"]),
    (&["oscilloscope", "waveform", "scope"], &["scope"]),
    (&["stop", "silence", "mute"], &["stop"]),
    (&["pause", "freeze", "hold"], &["pause"]),
    (&["example", "demo", "starter", "template"], &["example"]),
    (&["install", "download"], &["install", "package"]),
];

/// The Fugue words `word` (normalized) also searches for.
pub(super) fn of(word: &str) -> &'static [String] {
    static TABLE: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table: HashMap<String, Vec<String>> = HashMap::new();
        for (heard, meant) in SYNONYMS {
            for heard in *heard {
                let widened = table.entry(singular(heard)).or_default();
                for meant in *meant {
                    for meant in super::rank::all_words(meant) {
                        if !widened.contains(&meant) {
                            widened.push(meant);
                        }
                    }
                }
            }
        }
        table
    });
    table.get(word).map_or(&[], Vec::as_slice)
}
