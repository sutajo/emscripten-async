#![cfg(target_os = "emscripten")]

use emscripten_futures::{task, test};
use std::{cell::Cell, rc::Rc, time::Duration};

#[test]
async fn waits_for_timer_with_non_send_state() {
    let value = Rc::new(Cell::new(0));
    let shared = value.clone();
    task::sleep(Duration::from_millis(2)).await;
    shared.set(42);
    assert_eq!(value.get(), 42);
}

#[emscripten_futures::test]
async fn preserves_result_and_question_mark() -> Result<(), std::num::ParseIntError> {
    task::sleep(Duration::ZERO).await;
    assert_eq!("42".parse::<u32>()?, 42);
    Ok(())
}

#[emscripten_futures::test]
#[should_panic(expected = "async test panic")]
async fn preserves_should_panic() {
    task::sleep(Duration::ZERO).await;
    panic!("async test panic");
}

#[ignore = "checks that attributes survive macro expansion"]
#[emscripten_futures::test]
async fn preserves_ignore() {
    task::sleep(Duration::ZERO).await;
}

#[emscripten_futures::test]
#[cfg(any())]
async fn preserves_cfg() {
    this_must_not_be_compiled();
}
