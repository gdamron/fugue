//! Making an authored control write and recording it, when a timed edit may
//! land between the two (see `LiveGraph::record_authored`).

use super::RuntimeSnapshot;
use crate::control_request::RequestId;
use crate::invention::audio_assets;
use crate::invention::declared::{DeclaredSurface, Queued};
use crate::invention::publish::Authored;
use crate::invention::runtime::GraphCommandError;
use crate::ControlValue;

/// How many times an authored write is made when a timed edit is adopted
/// between making it and recording it.
pub(crate) const WRITE_ATTEMPTS: usize = 3;

/// Makes an authored write (`write`, which says the publisher generation
/// it was queued at when it knows, or else the one `generation` read just
/// before it, and the timed edit it is safe from) and records it
/// (`record`, given those), up to
/// `attempts` times while a timed edit overtakes it: lands after it, and
/// rebuilds or removes its module or writes the control itself (see
/// `LiveGraph::record_authored`). A write overtaken is never recorded, as
/// what plays may not be the write: overtaken on its last attempt, or
/// refused when made again, it is reported to be made again
/// (`ControlError`), with nothing recorded.
///
/// Concurrent authored writers are serialized by the caller
/// (`LiveGraph::authoring`), so the last recorded is the last to play.
pub(crate) fn write_authored(
    attempts: usize,
    target: (&str, &str),
    mut generation: impl FnMut() -> u64,
    mut write: impl FnMut() -> Result<Queued, GraphCommandError>,
    mut record: impl FnMut((u64, Option<RequestId>)) -> Authored,
) -> Result<(), GraphCommandError> {
    debug_assert!(attempts > 0, "an authored write is made at least once");
    let (module_id, key) = target;
    let overtaken = || {
        format!(
            "{module_id}.{key} was written, but a timed edit landed after it and rebuilt or \
             removed {module_id}, or wrote {module_id}.{key} itself, so it may not be playing; \
             nothing was recorded: make the write again"
        )
    };
    for attempt in 1..=attempts {
        let before = generation();
        let queued = match write() {
            Ok(queued) => queued,
            Err(error) if attempt == 1 => return Err(error),
            Err(error) => {
                let refused = format!("{}, and making it again was refused: {error}", overtaken());
                return Err(GraphCommandError::ControlError(refused));
            }
        };
        if record((queued.generation.unwrap_or(before), queued.safe)) == Authored::Recorded {
            return Ok(());
        }
    }
    Err(GraphCommandError::ControlError(overtaken()))
}

impl RuntimeSnapshot {
    /// Makes an authored write as [`Self::set_control_transient`] does,
    /// saying what it saw as it was queued when its module declares its
    /// controls (see `declared::Queued`).
    pub(crate) fn make_authored(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<Queued, GraphCommandError> {
        let surface = {
            let controls = self.control_surfaces.lock().unwrap();
            let surface = controls.get(module_id).cloned();
            surface.ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?
        };
        let declared = surface
            .as_any()
            .and_then(|any| any.downcast_ref::<DeclaredSurface>());
        let Some(declared) = declared else {
            self.set_control_transient(module_id, key, value)?;
            return Ok(Queued::default());
        };
        declared
            .set_control_queued(key, value, true)
            .map_err(GraphCommandError::ControlError)
    }

    /// The value a write to `key` hands its module: a relative path written
    /// to an asset control resolved against the invention's directory, as
    /// the loader resolves the document's, or refused when there is none
    /// (see `audio_assets::resolve_asset_write`). The document records the
    /// value as written.
    pub(super) fn asset_to_load(
        &self,
        key: &str,
        value: &ControlValue,
    ) -> Result<ControlValue, GraphCommandError> {
        if !audio_assets::is_asset_control(key) {
            return Ok(value.clone());
        }
        let source_path = {
            let state = self.state.lock().unwrap();
            state
                .document
                .as_ref()
                .and_then(|document| document.source_path.clone())
        };
        audio_assets::resolve_asset_write(source_path.as_deref(), key, value)
            .map_err(GraphCommandError::ControlError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: (&str, &str) = ("osc1", "frequency");

    /// A write overtaken by a timed edit, then refused on its retry (the
    /// queue full): nothing is recorded, and the caller is told to make it
    /// again.
    #[test]
    fn a_write_overtaken_then_refused_is_never_recorded() {
        let mut writes = 0;
        let mut records = Vec::new();
        let ended = write_authored(
            WRITE_ATTEMPTS,
            TARGET,
            || 7,
            || {
                writes += 1;
                match writes {
                    1 => Ok(Queued::default()),
                    _ => Err(GraphCommandError::QueueFull),
                }
            },
            |seen| {
                records.push(seen);
                Authored::Overtaken
            },
        );
        let Err(GraphCommandError::ControlError(message)) = ended else {
            panic!("expected a ControlError, got {ended:?}");
        };
        assert!(message.contains("make the write again"), "{message}");
        assert!(message.contains("refused"), "{message}");
        assert_eq!(records, [(7, None)]);
    }

    /// A write refused the first time is never recorded, and its refusal
    /// is returned as it is.
    #[test]
    fn a_write_never_made_is_never_recorded() {
        let mut recorded = false;
        let ended = write_authored(
            WRITE_ATTEMPTS,
            TARGET,
            || 0,
            || Err(GraphCommandError::QueueFull),
            |_| {
                recorded = true;
                Authored::Recorded
            },
        );
        assert!(matches!(ended, Err(GraphCommandError::QueueFull)) && !recorded);
    }

    /// Overtaken each time, the write is never recorded and is reported to
    /// be made again; recorded at once, it is made once.
    #[test]
    fn a_write_is_made_again_only_while_overtaken() {
        let mut writes = 0;
        let ended = write_authored(
            WRITE_ATTEMPTS,
            TARGET,
            || 0,
            || {
                writes += 1;
                Ok(Queued::default())
            },
            |_| Authored::Overtaken,
        );
        assert!(matches!(ended, Err(GraphCommandError::ControlError(_))));
        assert_eq!(writes, WRITE_ATTEMPTS);

        let mut writes = 0;
        let ended = write_authored(
            WRITE_ATTEMPTS,
            TARGET,
            || 0,
            || {
                writes += 1;
                Ok(Queued::default())
            },
            |_| Authored::Recorded,
        );
        assert_eq!((ended.unwrap(), writes), ((), 1));
    }
}
