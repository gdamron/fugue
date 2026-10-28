//! The request channel's backlog on a graph with no clock: reserved
//! requests that find the queue full are held, and a settle queues them.

use std::sync::atomic::Ordering;

use super::null_backend::{null_linked, OSCILLATOR};
use super::*;
use crate::control_request::{Request, RequestValue, RtValue, BACKLOG_CAPACITY};
use crate::invention::publish::publisher::QUEUE_SLOTS;
use crate::ControlValue;

/// A reserved write of `value` to the oscillator's frequency.
fn reserved(live: &LiveGraph, value: f32) -> Request {
    let publisher = live.publisher().lock().unwrap();
    let surface = live.control_surfaces.lock().unwrap()["osc"].clone();
    let index = surface.declaration("frequency").unwrap().index;
    let target = publisher.control_target("osc", index).unwrap();
    let mut request = Request::new(target, RequestValue::Value(RtValue::F32(value)));
    request.reserved = true;
    request
}

/// On a graph a [`crate::NullBackend`] renders, a settle renders until what
/// the channel held for want of room has been queued, so the last write
/// made before it is what plays when it returns.
#[test]
fn a_settle_queues_what_the_channel_held() {
    let (live, _backend, _render) = null_linked(OSCILLATOR);
    let surface = live.control_surfaces.lock().unwrap()["osc"].clone();
    let writes = QUEUE_SLOTS + BACKLOG_CAPACITY / 2;
    for n in 0..writes {
        let request = reserved(&live, 100.0 + n as f32);
        live.requests.submit(request).unwrap();
    }
    assert!(live.requests.holding().load(Ordering::Acquire), "held");
    live.settle();
    assert!(!live.requests.holding().load(Ordering::Acquire));
    let last = 100.0 + (writes - 1) as f32;
    assert_eq!(
        surface.get_control("frequency").unwrap(),
        ControlValue::Number(last)
    );
}
