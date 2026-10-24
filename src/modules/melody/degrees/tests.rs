use super::*;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

/// The selection algorithm before the table, kept as a reference. Only the
/// weights of the degrees in play count.
fn reference_choose(degrees: &[i32], weights: &[f32], unit: f32) -> i32 {
    let total_weight: f32 = (0..degrees.len())
        .map(|i| weights.get(i).copied().unwrap_or(1.0))
        .sum();
    let mut random_value = unit * total_weight;
    for (i, &degree) in degrees.iter().enumerate() {
        let weight = weights.get(i).unwrap_or(&1.0);
        if random_value < *weight {
            return degree;
        }
        random_value -= weight;
    }
    degrees[0]
}

fn table(scale: &[i32], weights: &[f32]) -> Degrees {
    let mut table = Degrees::new(scale);
    table.set_weights(weights);
    table
}

/// What the positions in play hold.
fn playing(table: &Degrees) -> (Vec<i32>, Vec<f32>) {
    let count = table.count();
    let degrees = (0..count).map(|i| table.degree(i)).collect();
    let weights = (0..count).map(|i| table.weight(i)).collect();
    (degrees, weights)
}

fn degrees(table: &Degrees) -> Vec<i32> {
    playing(table).0
}

fn weights(table: &Degrees) -> Vec<f32> {
    playing(table).1
}

#[test]
fn choose_matches_the_reference_algorithm() {
    let mut rng = StdRng::seed_from_u64(0xFEED);
    for _ in 0..500 {
        let degree_len = rng.random_range(1..=12);
        // Weight lists shorter, equal and longer than the degree list.
        let weight_len = rng.random_range(0..=16);
        let scale: Vec<i32> = (0..degree_len)
            .map(|_| rng.random_range(-24..=24))
            .collect();
        let given: Vec<f32> = (0..weight_len)
            .map(|_| match rng.random_bool(0.2) {
                true => 0.0,
                false => rng.random_range(0.0..10.0),
            })
            .collect();
        let table = table(&scale, &given);
        for _ in 0..20 {
            let unit: f32 = rng.random();
            assert_eq!(
                table.choose(unit),
                reference_choose(&scale, &given, unit),
                "scale {scale:?} weights {given:?} unit {unit}"
            );
        }
    }
}

#[test]
fn zero_weights_are_never_chosen() {
    let table = table(&[0, 4, 7], &[0.0, 1.0, 0.0]);
    for step in 0..100 {
        assert_eq!(table.choose(step as f32 / 100.0), 4);
    }
}

#[test]
fn shrinking_hides_positions_and_growing_shows_them_as_they_were() {
    let mut table = table(&[0, 2, 3, 5, 7, 9, 10], &[]);
    table.set_degree(6, 11);
    table.set_weight(5, 4.0);
    table.set_count(3);
    assert_eq!(degrees(&table), [0, 2, 3]);
    table.set_count(7);
    assert_eq!(degrees(&table), [0, 2, 3, 5, 7, 9, 11]);
    assert_eq!(table.weight(5), 4.0);
}

#[test]
fn growing_past_the_scale_repeats_its_degrees_and_weights() {
    let mut table = table(&[0, 4, 7], &[3.0, 1.0, 2.0]);
    table.set_count(8);
    assert_eq!(degrees(&table), [0, 4, 7, 0, 4, 7, 0, 4]);
    assert_eq!(weights(&table), [3.0, 1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0]);

    // A weight given past the scale is that position's own.
    table.set_weights(&[3.0, 1.0, 2.0, 9.0]);
    assert_eq!(weights(&table)[3..5], [9.0, 1.0]);
}

#[test]
fn an_empty_scale_plays_nothing_and_grows_with_degree_zero() {
    let mut table = Degrees::new(&[]);
    assert!(table.is_empty());
    table.set_count(3);
    assert_eq!(degrees(&table), [0, 0, 0]);
    table.set_degree(1, 7);
    assert_eq!(degrees(&table), [0, 7, 0]);
}

#[test]
fn lists_are_capped_at_max_degrees() {
    let long: Vec<i32> = (0..300).collect();
    let mut table = Degrees::new(&long);
    assert_eq!(table.count(), MAX_DEGREES);
    assert_eq!(table.degree(MAX_DEGREES - 1), MAX_DEGREES as i32 - 1);
    table.set_weights(&[2.0; 300]);
    assert_eq!(table.set_count(1000), MAX_DEGREES);
    assert_eq!(table.set_count(0), 1);
}

#[test]
fn single_positions_clamp_as_they_are_written() {
    let mut table = Degrees::new(&[0, 2]);
    assert_eq!(table.set_degree(0, 300), 127);
    assert_eq!(table.set_degree(1, -300), -127);
    assert_eq!(table.set_weight(0, 11.0), 10.0);
    assert_eq!(table.set_weight(1, -1.0), 0.0);
}

// The serial orders the concurrent writers of an earlier, lock-based table
// had to agree with: each now is just the order its requests apply in.

/// An 8-degree melody, as a document might configure one.
fn octave() -> Degrees {
    Degrees::new(&[0, 2, 4, 5, 7, 9, 11, 12])
}

#[test]
fn the_last_write_to_a_position_wins() {
    let mut table = octave();
    table.set_degree(1, 5);
    table.set_degree(1, 9);
    table.set_weight(1, 2.0);
    table.set_weight(1, 3.0);
    assert_eq!((table.degree(1), table.weight(1)), (9, 3.0));
}

#[test]
fn a_whole_scale_after_position_writes_wins() {
    let mut table = octave();
    table.set_degree(1, 9);
    table.set_scale(&[0, 4, 7]);
    assert_eq!(degrees(&table), [0, 4, 7]);
}

#[test]
fn a_weight_written_before_a_scale_replacement_is_kept_hidden() {
    let mut table = octave();
    table.set_weight(6, 4.0);
    table.set_scale(&[0, 4, 7]);
    assert_eq!(weights(&table), [1.0; 3]);
    table.set_count(7);
    assert_eq!(table.weight(6), 4.0);
}

#[test]
fn writes_before_a_shrink_are_kept_hidden_and_shown_by_growth() {
    let mut table = octave();
    table.set_degree(5, 3);
    table.set_weight(5, 4.0);
    table.set_count(2);
    assert_eq!(degrees(&table), [0, 2]);
    table.set_count(6);
    assert_eq!(degrees(&table), [0, 2, 4, 5, 7, 3]);
    assert_eq!(table.weight(5), 4.0);
}

#[test]
fn a_write_past_the_count_is_kept_hidden_until_it_grows() {
    let mut table = octave();
    table.set_count(2);
    table.set_degree(5, 3);
    assert_eq!(degrees(&table), [0, 2]);
    table.set_count(6);
    assert_eq!(degrees(&table), [0, 2, 4, 5, 7, 3]);
}

#[test]
fn a_scale_replacement_erases_a_degree_written_past_a_shrink() {
    let mut table = octave();
    table.set_degree(5, 3);
    table.set_count(2);
    table.set_scale(&[0, 4, 7]);
    assert_eq!(degrees(&table), [0, 4, 7]);
    table.set_count(6);
    assert_eq!(degrees(&table), [0, 4, 7, 0, 4, 7]);
}

#[test]
fn a_weight_replacement_erases_only_written_weights() {
    let mut table = octave();
    table.set_degree(5, 3);
    table.set_weight(5, 4.0);
    table.set_count(2);
    table.set_weights(&[2.0]);
    table.set_count(6);
    assert_eq!(degrees(&table), [0, 2, 4, 5, 7, 3]);
    assert_eq!(weights(&table), [2.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn writes_after_a_replacement_are_kept() {
    let mut table = octave();
    table.set_degree(1, 9);
    table.set_scale(&[0, 4, 7, 11]);
    table.set_weights(&[1.0]);
    table.set_degree(2, 5);
    table.set_weight(1, 3.0);
    assert_eq!(degrees(&table), [0, 4, 5, 11]);
    assert_eq!(weights(&table), [1.0, 3.0, 1.0, 1.0]);
}

#[test]
fn a_replacement_erases_a_degree_before_it_and_a_growth_after_repeats_it() {
    let mut table = octave();
    table.set_degree(5, 3);
    table.set_scale(&[0, 4, 7]);
    table.set_count(8);
    assert_eq!(degrees(&table), [0, 4, 7, 0, 4, 7, 0, 4]);
}

/// What a saved document records (the count and single-position writes)
/// rebuilds the table the live writes made, in any order.
#[test]
fn the_table_is_the_same_whatever_order_its_sources_are_set_in() {
    let mut live = Degrees::new(&[0, 2, 4, 5, 7, 9, 11]);
    live.set_degree(6, 1);
    live.set_count(3);
    live.set_degree(1, 3);
    live.set_count(10);
    live.set_weight(8, 5.0);
    live.set_count(9);

    let mut rebuilt = Degrees::new(&[0, 2, 4, 5, 7, 9, 11]);
    rebuilt.set_weight(8, 5.0);
    rebuilt.set_degree(1, 3);
    rebuilt.set_count(9);
    rebuilt.set_degree(6, 1);
    assert_eq!(playing(&rebuilt), playing(&live));
}
