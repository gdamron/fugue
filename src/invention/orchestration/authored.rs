//! Making an authored control write and recording it, when a timed edit may
//! land between the two (see `LiveGraph::record_authored`).

use crate::invention::publish::Authored;
use crate::invention::runtime::GraphCommandError;

/// How many times an authored write is made when a timed edit is adopted
/// between making it and recording it.
pub(super) const WRITE_ATTEMPTS: usize = 3;

/// How an authored write that was made and recorded ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Written {
    /// Recorded, and what plays.
    Recorded,
    /// Recorded on its last attempt, though a timed edit landed after it
    /// was made: the edit may have written it or replaced its module.
    Raced,
}

/// Makes an authored write (`write`, reading the publisher's generation
/// first with `generation`) and records it (`record`, given the generation
/// seen and whether this is the last attempt), up to `attempts` times while
/// a timed edit is adopted in between. A write made is always recorded:
/// when making it again fails, the attempt before is recorded before the
/// failure is returned, since it may be playing.
pub(super) fn write_authored(
    attempts: usize,
    mut generation: impl FnMut() -> u64,
    mut write: impl FnMut() -> Result<(), GraphCommandError>,
    mut record: impl FnMut(u64, bool) -> Authored,
) -> Result<Written, GraphCommandError> {
    let mut made = None;
    for attempt in 1..=attempts {
        let seen = generation();
        if let Err(error) = write() {
            if let Some(seen) = made {
                record(seen, true);
            }
            return Err(error);
        }
        match record(seen, attempt == attempts) {
            Authored::Recorded => return Ok(Written::Recorded),
            Authored::Raced => made = Some(seen),
            Authored::RecordedRaced => return Ok(Written::Raced),
        }
    }
    // A last attempt is never `Raced`, so the loop returns; one attempt
    // too few is reported as raced, never as unrecorded.
    Ok(Written::Raced)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A write made, raced by a timed edit, then refused on its retry (the
    /// queue full): the write made is still recorded, as the last attempt,
    /// and the refusal returned.
    #[test]
    fn a_write_made_is_recorded_though_its_retry_is_refused() {
        let mut writes = 0;
        let mut records = Vec::new();
        let ended = write_authored(
            WRITE_ATTEMPTS,
            || 7,
            || {
                writes += 1;
                match writes {
                    1 => Ok(()),
                    _ => Err(GraphCommandError::QueueFull),
                }
            },
            |seen, last| {
                records.push((seen, last));
                if last {
                    Authored::RecordedRaced
                } else {
                    Authored::Raced
                }
            },
        );
        assert!(matches!(ended, Err(GraphCommandError::QueueFull)));
        assert_eq!(records, [(7, false), (7, true)]);
    }

    /// A write refused the first time is never recorded.
    #[test]
    fn a_write_never_made_is_never_recorded() {
        let mut recorded = false;
        let ended = write_authored(
            WRITE_ATTEMPTS,
            || 0,
            || Err(GraphCommandError::QueueFull),
            |_, _| {
                recorded = true;
                Authored::Recorded
            },
        );
        assert!(ended.is_err() && !recorded);
    }

    /// Raced each time, the write is recorded on its last attempt and
    /// reported raced; recorded at once, it is made once.
    #[test]
    fn a_write_is_made_again_only_while_raced() {
        let mut writes = 0;
        let ended = write_authored(
            WRITE_ATTEMPTS,
            || 0,
            || {
                writes += 1;
                Ok(())
            },
            |_, last| {
                if last {
                    Authored::RecordedRaced
                } else {
                    Authored::Raced
                }
            },
        );
        assert_eq!((ended.unwrap(), writes), (Written::Raced, WRITE_ATTEMPTS));

        let mut writes = 0;
        let ended = write_authored(
            WRITE_ATTEMPTS,
            || 0,
            || {
                writes += 1;
                Ok(())
            },
            |_, _| Authored::Recorded,
        );
        assert_eq!((ended.unwrap(), writes), (Written::Recorded, 1));
    }
}
