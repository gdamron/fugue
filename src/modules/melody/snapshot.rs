//! Audio-thread copy of the melody's degree table.

use super::controls::{DegreeTable, MelodyControls, MAX_DEGREES};

/// Pre-allocated copy of the allowed degrees and their weights.
///
/// Owned by the audio thread. [`DegreeSnapshot::sync`] refreshes it from
/// [`MelodyControls`] only when the table version changed or a deferred
/// write is pending, using `try_lock` so it never blocks; if a control thread
/// holds the lock, the previous copy is kept and the sync is retried on the
/// next call.
pub(super) struct DegreeSnapshot {
    degrees: [i32; MAX_DEGREES],
    degree_count: usize,
    weights: [f32; MAX_DEGREES],
    weight_count: usize,
    /// Sum of the weights of the degrees in play.
    total_weight: f32,
    version: u64,
}

impl DegreeSnapshot {
    /// Builds a snapshot of the current table. Blocks on the lock, so call
    /// it at construction time, not from the audio thread.
    pub(super) fn new(ctrl: &MelodyControls) -> Self {
        let mut snapshot = Self {
            degrees: [0; MAX_DEGREES],
            degree_count: 0,
            weights: [0.0; MAX_DEGREES],
            weight_count: 0,
            total_weight: 0.0,
            version: 0,
        };
        let table = ctrl.lock_table();
        snapshot.version = ctrl.table_version();
        snapshot.copy_from(&table);
        snapshot
    }

    /// Re-copies the table if it changed, draining any deferred write first.
    /// Lock-free unless the version moved or a write is pending, and never
    /// blocks. Returns `true` if the snapshot was refreshed.
    pub(super) fn sync(&mut self, ctrl: &MelodyControls) -> bool {
        if !ctrl.has_pending() && ctrl.table_version() == self.version {
            return false;
        }
        // Locking drains deferred writes. Every edit bumps the version under
        // the lock, so the version read here matches the table copied.
        let Some(table) = ctrl.try_lock_table() else {
            return false;
        };
        let version = ctrl.table_version();
        if version == self.version {
            return false;
        }
        self.copy_from(&table);
        self.version = version;
        true
    }

    fn copy_from(&mut self, table: &DegreeTable) {
        self.degree_count = table.degrees.len().min(MAX_DEGREES);
        self.degrees[..self.degree_count].copy_from_slice(&table.degrees[..self.degree_count]);
        self.weight_count = table.weights.len().min(MAX_DEGREES);
        self.weights[..self.weight_count].copy_from_slice(&table.weights[..self.weight_count]);
        self.total_weight = table.weights.iter().sum();
    }

    /// Returns `true` if no degrees are allowed.
    pub(super) fn is_empty(&self) -> bool {
        self.degree_count == 0
    }

    /// Picks a degree by weighted choice. `unit` is a uniform random value in
    /// `[0, 1)`. Degrees without a weight count as weight 1.0; if the draw
    /// falls past every degree, the first degree is returned.
    ///
    /// Must not be called when [`Self::is_empty`].
    pub(super) fn choose(&self, unit: f32) -> i32 {
        let mut remaining = unit * self.total_weight;
        for i in 0..self.degree_count {
            let weight = if i < self.weight_count {
                self.weights[i]
            } else {
                1.0
            };
            if remaining < weight {
                return self.degrees[i];
            }
            remaining -= weight;
        }
        self.degrees[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    /// The pre-snapshot selection algorithm, kept as a reference. Only the
    /// weights of the degrees in play count: a weight past the scale belongs
    /// to a position the count has not grown to.
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

    #[test]
    fn choose_matches_reference_algorithm() {
        let mut rng = StdRng::seed_from_u64(0xFEED);
        for _ in 0..500 {
            let degree_len = rng.random_range(1..=12);
            // Weight lists shorter, equal and longer than the degree list.
            let weight_len = rng.random_range(0..=16);
            let degrees: Vec<i32> = (0..degree_len)
                .map(|_| rng.random_range(-24..=24))
                .collect();
            let weights: Vec<f32> = (0..weight_len)
                .map(|_| {
                    if rng.random_bool(0.2) {
                        0.0
                    } else {
                        rng.random_range(0.0..10.0)
                    }
                })
                .collect();

            let ctrl = MelodyControls::new(60, degrees.clone());
            ctrl.set_note_weights(weights.clone());
            let snapshot = DegreeSnapshot::new(&ctrl);

            for _ in 0..20 {
                let unit: f32 = rng.random();
                assert_eq!(
                    snapshot.choose(unit),
                    reference_choose(&degrees, &weights, unit),
                    "degrees {degrees:?} weights {weights:?} unit {unit}"
                );
            }
        }
    }

    #[test]
    fn zero_weights_are_never_chosen() {
        let ctrl = MelodyControls::new(60, vec![0, 4, 7]);
        ctrl.set_note_weights(vec![0.0, 1.0, 0.0]);
        let snapshot = DegreeSnapshot::new(&ctrl);
        for step in 0..100 {
            assert_eq!(snapshot.choose(step as f32 / 100.0), 4);
        }
    }

    #[test]
    fn sync_is_a_no_op_until_the_table_changes() {
        let ctrl = MelodyControls::new(60, vec![0, 2, 4]);
        let mut snapshot = DegreeSnapshot::new(&ctrl);
        assert!(!snapshot.sync(&ctrl));

        // Scalar controls don't touch the table.
        ctrl.set_root_note(62);
        ctrl.set_seed(3);
        assert!(!snapshot.sync(&ctrl));

        ctrl.set_degree(1, 5).unwrap();
        assert!(snapshot.sync(&ctrl));
        assert_eq!(snapshot.degrees[..snapshot.degree_count], [0, 5, 4]);
        assert!(!snapshot.sync(&ctrl));
    }

    #[test]
    fn every_table_edit_is_picked_up() {
        let ctrl = MelodyControls::new(60, vec![0, 2, 4]);
        let mut snapshot = DegreeSnapshot::new(&ctrl);

        ctrl.set_allowed_degrees(vec![7, 9]);
        assert!(snapshot.sync(&ctrl));
        assert_eq!(snapshot.degrees[..snapshot.degree_count], [7, 9]);
        assert_eq!(snapshot.weights[..snapshot.weight_count], [1.0, 1.0]);

        ctrl.set_note_weights(vec![0.0, 3.0]);
        assert!(snapshot.sync(&ctrl));
        assert_eq!(snapshot.total_weight, 3.0);
        assert_eq!(snapshot.choose(0.0), 9);

        ctrl.set_note_weight(0, 2.0).unwrap();
        assert!(snapshot.sync(&ctrl));
        assert_eq!(snapshot.weights[..snapshot.weight_count], [2.0, 3.0]);

        ctrl.set_degree_count(4);
        assert!(snapshot.sync(&ctrl));
        assert_eq!(snapshot.degrees[..snapshot.degree_count], [7, 9, 7, 9]);
    }

    #[test]
    fn sync_does_not_block_while_a_control_thread_holds_the_lock() {
        let ctrl = MelodyControls::new(60, vec![0, 2, 4]);
        let mut snapshot = DegreeSnapshot::new(&ctrl);
        ctrl.set_allowed_degrees(vec![11]);

        let guard = ctrl.table.lock().unwrap();
        // Contended: keep the stale copy rather than wait.
        assert!(!snapshot.sync(&ctrl));
        assert_eq!(snapshot.degrees[..snapshot.degree_count], [0, 2, 4]);
        drop(guard);

        // The pending change is applied on the next uncontended sync.
        assert!(snapshot.sync(&ctrl));
        assert_eq!(snapshot.degrees[..snapshot.degree_count], [11]);
    }

    #[test]
    fn degree_lists_are_capped_at_max_degrees() {
        let ctrl = MelodyControls::new(60, (0..200).collect());
        assert_eq!(ctrl.degree_count(), MAX_DEGREES);
        ctrl.set_allowed_degrees((0..300).collect());
        assert_eq!(ctrl.degree_count(), MAX_DEGREES);

        let snapshot = DegreeSnapshot::new(&ctrl);
        assert_eq!(snapshot.degree_count, MAX_DEGREES);
        assert_eq!(snapshot.degrees[MAX_DEGREES - 1], MAX_DEGREES as i32 - 1);
    }

    #[test]
    fn empty_table_is_reported() {
        let ctrl = MelodyControls::new(60, vec![]);
        assert!(DegreeSnapshot::new(&ctrl).is_empty());
    }
}
