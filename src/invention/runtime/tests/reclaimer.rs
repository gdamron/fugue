//! A reclaimer thread that cannot start is a start that fails, never a
//! runtime whose held note offs wait for a later write.

use super::*;

#[test]
fn a_started_reclaimer_lets_the_runtime_start() {
    assert!(reclaimer_started(Ok(())).is_ok());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_reclaimer_that_cannot_start_fails_the_start_with_its_reason() {
    let error = reclaimer_started(Err(std::io::Error::other("no threads left")))
        .expect_err("the start must fail");

    assert_eq!(
        error.to_string(),
        "could not start the reclaimer thread: no threads left"
    );
}
