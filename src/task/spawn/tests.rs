#![allow(
    clippy::waker_clone_wake,
    reason = "Test both owned and borrowed wake paths"
)]

use super::spawn_local;
use crate::{executor::block_on, task::sleep};
use futures::{
    channel::oneshot::{self, Canceled},
    future::poll_fn,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    task::Poll,
    time::Duration,
};

#[test]
fn block_on_awaits_two_spawned_tasks() {
    let results = block_on(async {
        let (first_send, first_recv) = oneshot::channel();
        let (second_send, second_recv) = oneshot::channel::<()>();

        spawn_local(async move {
            sleep(Duration::from_millis(1)).await;
            first_send.send(21).unwrap();
        });
        spawn_local(async move {
            sleep(Duration::from_millis(2)).await;
            let _second_send = second_send;
            sleep(Duration::from_millis(2)).await;
            sleep(Duration::from_millis(2)).await;
            sleep(Duration::from_millis(2)).await;
            sleep(Duration::from_millis(2)).await;
        });

        // spawn_local is detached, so await each task's result through a channel.
        let (first, second) = futures::join!(first_recv, second_recv);
        (first.unwrap(), second.unwrap_err())
    });

    assert_eq!(results, (21, Canceled));
}

#[test]
fn defers_first_poll_and_runs_before_timers() {
    let ran = Rc::new(Cell::new(false));
    let task_ran = ran.clone();
    spawn_local(async move { task_ran.set(true) });
    assert!(!ran.get(), "spawn_local polled inline");
    block_on(sleep(Duration::ZERO));
    assert!(ran.get(), "microtask did not run before the timer");
}

#[crate::test]
async fn resumes_non_send_future_after_timer() {
    let state = Rc::new(Cell::new(0));
    let task_state = state.clone();
    let (send, mut recv) = oneshot::channel();
    spawn_local(async move {
        task_state.set(1);
        sleep(Duration::from_millis(1)).await;
        task_state.set(2);
        send.send(()).unwrap();
    });
    // Bound the wait so a lost wakeup fails rather than hanging the test suite.
    sleep(Duration::from_millis(30)).await;
    assert_eq!(recv.try_recv().unwrap(), Some(()));
    assert_eq!(state.get(), 2);
    assert_eq!(Rc::strong_count(&state), 1);
}

#[crate::test]
async fn coalesces_wakes_and_ignores_wakes_after_completion() {
    let polls = Rc::new(Cell::new(0));
    let task_polls = polls.clone();
    let retained_waker = Rc::new(RefCell::new(None));
    let task_waker = retained_waker.clone();
    let state = Rc::new(());
    let weak = Rc::downgrade(&state);
    spawn_local(async move {
        poll_fn(|cx| {
            let poll = task_polls.get() + 1;
            task_polls.set(poll);
            *task_waker.borrow_mut() = Some(cx.waker().clone());
            if poll == 1 || poll == 3 {
                cx.waker().wake_by_ref();
                cx.waker().clone().wake();
            }
            // The second poll waits for an external wake. Duplicate queued
            // callbacks from the first poll must not cause a third poll yet.
            if poll < 3 {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        drop(state);
    });
    sleep(Duration::ZERO).await;
    assert_eq!(polls.get(), 2);
    assert!(weak.upgrade().is_some());
    let waker = retained_waker.borrow_mut().take().unwrap();
    waker.wake_by_ref();
    waker.wake_by_ref();
    sleep(Duration::ZERO).await;
    assert_eq!(polls.get(), 3);
    assert!(weak.upgrade().is_none(), "completed future was retained");
    waker.wake();
    sleep(Duration::ZERO).await;
    assert_eq!(polls.get(), 3);
}

#[crate::test]
async fn can_spawn_from_a_task_and_its_destructor() {
    struct SpawnOnDrop(Rc<Cell<usize>>);
    impl Drop for SpawnOnDrop {
        fn drop(&mut self) {
            let count = self.0.clone();
            spawn_local(async move { count.set(count.get() + 1) });
        }
    }
    let count = Rc::new(Cell::new(0));
    let task_count = count.clone();
    let on_drop = SpawnOnDrop(count.clone());
    // A poll_fn retains captures until the executor drops the completed future.
    spawn_local(poll_fn(move |_| {
        let _ = &on_drop;
        let count = task_count.clone();
        spawn_local(async move { count.set(count.get() + 1) });
        Poll::Ready(())
    }));
    sleep(Duration::ZERO).await;
    assert_eq!(count.get(), 2);
}

#[crate::test]
async fn ignores_wakes_after_completion_without_a_self_wake() {
    let polls = Rc::new(Cell::new(0));
    let task_polls = polls.clone();
    let retained_waker = Rc::new(RefCell::new(None));
    let task_waker = retained_waker.clone();
    spawn_local(poll_fn(move |cx| {
        task_polls.set(task_polls.get() + 1);
        *task_waker.borrow_mut() = Some(cx.waker().clone());
        Poll::Ready(())
    }));

    sleep(Duration::ZERO).await;
    assert_eq!(polls.get(), 1);
    assert_eq!(Rc::strong_count(&polls), 1, "completed future was retained");
    let waker = retained_waker.borrow_mut().take().unwrap();
    waker.wake_by_ref();
    waker.wake();
    sleep(Duration::ZERO).await;
    assert_eq!(polls.get(), 1);
}
