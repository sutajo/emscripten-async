#![cfg(target_os = "emscripten")]
#![feature(test)]

extern crate test;

use emscripten_futures::{bench, task};
use std::{cell::Cell, rc::Rc};

#[bench]
async fn waits_with_non_send_state() -> Rc<Cell<u32>> {
    let state = Rc::new(Cell::new(0));
    let shared = state.clone();
    task::yield_now().await;
    shared.set(42);
    assert_eq!(state.get(), 42);
    state
}

#[bench]
async fn preserves_return_type_and_question_mark() -> Result<u32, std::num::ParseIntError> {
    let value = "42".parse::<u32>()?;
    task::yield_now().await;
    Ok(value)
}

#[bench]
async fn unit_return() {
    task::yield_now().await;
}

#[bench]
#[ignore = "checks that attributes survive macro expansion"]
async fn preserves_ignore() {
    panic!("ignored benchmark ran");
}

#[bench]
#[cfg(any())]
async fn preserves_cfg() {
    this_must_not_be_compiled();
}
