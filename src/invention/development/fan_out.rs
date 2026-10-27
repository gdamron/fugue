//! A payload one development key hands to several inner controls: a
//! sample written to every player behind one `sample`.
//!
//! Only a payload that loads what its value names (`loads_on_write`, a
//! sample file) fans out: each alias's own surface builds its own payload
//! from the value on the control thread ([`fanned_out`]), and the audio
//! thread hands each to its alias, keeping what each replaces in the same
//! box, which it retires whole ([`apply_fanned_out`]). Nothing is built or
//! freed there. A payload carrying more than its value (a scheduler's
//! resolved schedule, built against the whole development) reaches one
//! alias alone.

use crate::control_request::Refusal;
use crate::payload::{LeakOnUnwind, Payload, Retired};

/// One payload per alias, in alias order, and what each replaced.
struct FannedOut {
    slots: Box<[Slot]>,
}

struct Slot {
    payload: Option<Payload>,
    replaced: Option<Retired>,
}

/// Packs `payloads`, one per alias in alias order. Control thread:
/// allocates.
pub(super) fn fanned_out(payloads: Vec<Payload>) -> Payload {
    let slots = (payloads.into_iter())
        .map(|payload| Slot {
            payload: Some(payload),
            replaced: None,
        })
        .collect();
    Payload::owned(Box::new(FannedOut { slots }))
}

/// Hands each of `payload`'s payloads to its alias with `apply`, which
/// takes the alias's position and the payload. A payload packed for
/// another count of aliases is refused whole, before any applies. Past
/// that, every alias applies, and the first refusal is returned with the
/// box, which then holds what the others replaced: the caller retires it
/// either way. Allocation- and free-free: the audio thread applies
/// payloads.
pub(super) fn apply_fanned_out(
    aliases: usize,
    payload: Payload,
    mut apply: impl FnMut(usize, Payload) -> Result<Retired, (Refusal, Payload)>,
) -> Result<Retired, (Refusal, Payload)> {
    let mut fanned = match payload.take_owned::<FannedOut>() {
        Ok(fanned) => fanned,
        Err(payload) => return Err((Refusal::Invalid, payload)),
    };
    if fanned.slots.len() != aliases || fanned.slots.iter().any(|slot| slot.payload.is_none()) {
        return Err((Refusal::Invalid, Payload::owned(fanned)));
    }
    // An alias panicking leaves the box, and what the others replaced, to
    // leak rather than free here.
    let mut fanned = LeakOnUnwind::new(fanned);
    let mut refused = None;
    for (position, slot) in fanned.slots.iter_mut().enumerate() {
        let Some(payload) = slot.payload.take() else {
            continue;
        };
        match apply(position, payload) {
            Ok(replaced) => slot.replaced = Some(replaced),
            Err((refusal, payload)) => {
                slot.payload = Some(payload);
                refused.get_or_insert(refusal);
            }
        }
    }
    let fanned = fanned.into_inner();
    match refused {
        None => Ok(Retired::from(fanned)),
        Some(refusal) => Err((refusal, Payload::owned(fanned))),
    }
}
