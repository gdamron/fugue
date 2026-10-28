//! Where a ramp in beats starts: at its beat, a watch hands its request
//! to the ramps (see `super::super::ramps`).

use super::{BeatHost, Watches};
use crate::control_request::hold::Standing;
use crate::control_request::pending::{Outcome, Outcomes, Refusal};
use crate::control_request::ramps::BeatRamp;
use crate::control_request::request::{RequestValue, RtValue, When};

impl Watches {
    /// The watches carrying a ramp: each has room taken in the ramps.
    pub(crate) fn ramps_waiting(&self) -> usize {
        let ramps = |watch: &&super::Watch| match watch.request.when {
            When::Beat(beat) => beat.ramp.is_some(),
            _ => false,
        };
        self.entries.iter().filter(ramps).count()
    }

    /// Starts the ramp watch `i` carries: its beat `beat` (in its clock's
    /// positions now) was reached at sample `due`, and it applies at `now`.
    /// Writing the control's own value, it takes the control as a request
    /// would, and reports so; the ramp then runs from that value among
    /// the ramps. One whose control holds no finite number is refused
    /// ([`Refusal::Unsupported`]). Either way the watch ends.
    /// Allocation- and free-free.
    pub(super) fn start_ramp(
        &mut self,
        i: usize,
        (beat, due, now): (f64, u64, u64),
        host: &mut impl BeatHost,
        outcomes: &mut Outcomes,
    ) {
        let watch = self.entries.remove(i);
        let (When::Beat(time), &RequestValue::Value(RtValue::F32(to))) =
            (watch.request.when, &watch.request.value)
        else {
            unreachable!("Watches::check lets only a number ramp through");
        };
        let Some(beats) = time.ramp else {
            unreachable!("only a ramp starts one");
        };
        let target = watch.request.target;
        let standing = Standing::of(&watch.request, due);
        let result = match (host.current(&target), standing) {
            (Some(from), Some(_)) if from.is_finite() => {
                let value = RequestValue::Value(RtValue::F32(from));
                outcomes.applying(super::on_beat(watch.request.id, due, now));
                host.apply(&target, standing, value, outcomes)
                    .map(|()| from)
            }
            _ => Err(Outcome::Refused(Refusal::Unsupported)),
        };
        let outcome = match result {
            Ok(_) if due < now => Outcome::AppliedLate { at: now, due },
            Ok(_) => Outcome::Applied { at: now },
            Err(outcome) => outcome,
        };
        let latched = (due == now && result.is_err()).then_some(now);
        outcomes.record_on_beat(watch.request.id, false, outcome, latched, true);
        let (Ok(from), Some(standing)) = (result, standing) else {
            return;
        };
        let begun = host
            .timeline(watch.clock)
            .map_or(0, |timeline| timeline.beats_before());
        let ramp = BeatRamp::new(from, to, beat, f64::from(beats), begun);
        self.ramps
            .start((target, watch.clock), standing, ramp, &*host);
    }
}
