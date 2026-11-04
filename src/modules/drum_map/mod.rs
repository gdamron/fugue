//! Drum map: kick, snare and hat patterns from a navigable map of grooves.
//!
//! The map is a 3 x 3 grid of one-bar patterns we authored (see
//! `scripts/generate_drum_map.py`). `x` moves across groove families
//! (straight, syncopated, broken) and `y` up in energy (sparse half-time,
//! standard, driving); between nodes the neighbours blend. [`map`] answers
//! what each voice plays on a step.

pub mod map;
mod nodes;
