//! Making an authored control write and recording it, when a timed edit may
//! land between the two (see `LiveGraph::record_authored`).

use crate::invention::publish::Authored;
use crate::invention::runtime::GraphCommandError;

/// How many times an authored write is made when a timed edit is adopted
/// between making it and recording it.
pub(crate) const WRITE_ATTEMPTS: usize = 3;

/// Makes an authored write (`write`, reading the publisher's generation
/// first with `generation`) and records it (`record`, given the generation
/// seen), up to `attempts` times while a timed edit overtakes it: lands
/// after it is made, and rebuilds or removes its module (see
/// `LiveGraph::record_authored`). A write overtaken is never recorded, as
/// what plays may not be the write: overtaken on its last attempt, or
/// refused when made again, it is reported to be made again
/// (`ControlError`), with nothing recorded.
///
/// Concurrent authored writers to one control are not serialized, as they
/// are not on main: the last to be recorded may not be the last to play.
pub(crate) fn write_authored(
    attempts: usize,
    target: (&str, &str),
    mut generation: impl FnMut() -> u64,
    mut write: impl FnMut() -> Result<(), GraphCommandError>,
    mut record: impl FnMut(u64) -> Authored,
) -> Result<(), GraphCommandError> {
    debug_assert!(attempts > 0, "an authored write is made at least once");
    let (module_id, key) = target;
    let overtaken = || {
        format!(
            "{module_id}.{key} was written, but a timed edit landed after it and rebuilt or \
             removed {module_id}, so it may not be playing; nothing was recorded: make the \
             write again"
        )
    };
    for attempt in 1..=attempts {
        let seen = generation();
        if let Err(error) = write() {
            if attempt == 1 {
                return Err(error);
            }
            let refused = format!("{}, and making it again was refused: {error}", overtaken());
            return Err(GraphCommandError::ControlError(refused));
        }
        if record(seen) == Authored::Recorded {
            return Ok(());
        }
    }
    Err(GraphCommandError::ControlError(overtaken()))
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
                    1 => Ok(()),
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
        assert_eq!(records, [7]);
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
                Ok(())
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
                Ok(())
            },
            |_| Authored::Recorded,
        );
        assert_eq!((ended.unwrap(), writes), ((), 1));
    }
}
