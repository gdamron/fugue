//! How a publication's modules relate to the graph running when it installs.

use indexmap::IndexMap;

use crate::invention::runtime::ModuleInstance;

/// The survivor remap: for each module of the graph that is running when a
/// publication installs, in that graph's module order (its *old* index), the
/// index in the publication's module order (its *new* index) of the same
/// surviving instance, or `None` when the publication removes the module or
/// replaces it with a newly built instance.
///
/// This is the shared contract for carrying per-module audio-thread state
/// across a publication. It is built and sized on the control thread; the
/// audio thread only reads it, so using it during install never allocates,
/// frees, or locks. Install uses it to carry each survivor's previous-block
/// outputs (the feedback carry), so an edit that does not touch a feedback
/// loop leaves the loop sample-identical while added and rebuilt modules
/// start from zero, and to compare each survivor's input connectivity
/// before and after, so its inputs that stay unconnected keep their values. (Sample-identical as long as the edit leaves the loop's
/// per-sample order unchanged: which edge of a loop is delayed follows module
/// order and the edges into the loop, so an edit that moves the loop's entry
/// point changes which edge reads the carry.) It is general on purpose: any
/// state the audio thread keeps by module index (queued input writes,
/// re-binding surviving schedulers) can follow survivors through it rather
/// than by id.
///
/// It maps one way only, old to new, and lives in the publication, so it
/// describes the step at install and nothing else. Anything resolved against
/// the publisher's mirror refers to that generation's order, so a consumer
/// must know which generation it was resolved against: queued input writes
/// and control requests carry it.
///
/// Old indices are relative to what is actually running at install, not to
/// what the publication was prepared against: the publisher maps a
/// publication against its mirror of the running order as it publishes, and
/// edits install in the order they were published, so that is the order
/// the audio thread is running when it installs this one.
///
/// A remap that does not cover the running graph (a default, unmapped one,
/// say) carries nothing; install checks its length before using it.
#[derive(Clone, Debug, Default)]
pub(crate) struct SurvivorRemap {
    /// New index by old index.
    new_of_old: Vec<Option<usize>>,
}

impl SurvivorRemap {
    /// Maps the modules `running` (ids in running order) onto `next`, a
    /// publication's module map, where `survivor[new]` marks the entries the
    /// running instance will fill. Control thread only: allocates.
    pub(crate) fn map<'a>(
        running: impl IntoIterator<Item = &'a str>,
        next: &IndexMap<String, ModuleInstance>,
        survivor: &[bool],
    ) -> Self {
        let new_of_old = running
            .into_iter()
            .map(|id| {
                next.get_index_of(id)
                    .filter(|&new| survivor.get(new).copied().unwrap_or(false))
            })
            .collect();
        Self { new_of_old }
    }

    /// Number of running modules the remap covers.
    pub(crate) fn len(&self) -> usize {
        self.new_of_old.len()
    }

    /// The new index of the running module at `old`, if it survives.
    pub(crate) fn get(&self, old: usize) -> Option<usize> {
        self.new_of_old.get(old).copied().flatten()
    }

    /// `(old, new)` for every surviving module, in old order. Allocation-free.
    pub(crate) fn survivors(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.new_of_old
            .iter()
            .enumerate()
            .filter_map(|(old, new)| new.map(|new| (old, new)))
    }
}
