#![allow(
    clippy::waker_clone_wake,
    reason = "Test both owned and borrowed wake paths"
)]

use super::spawn_local;
use crate::{executor::block_on, task::sleep};
use futures::{
    FutureExt,
    channel::oneshot::{self, Canceled},
    future::poll_fn,
};
use std::{
    cell::{Cell, RefCell},
    panic::AssertUnwindSafe,
    rc::Rc,
    task::{Poll, Waker},
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

#[crate::test]
async fn drops_panicking_tasks_and_ignores_their_wakes() {
    for self_wake in [false, true] {
        let polls = Rc::new(Cell::new(0));
        let task_polls = polls.clone();
        let retained_waker = Rc::new(RefCell::new(None));
        let task_waker = retained_waker.clone();
        // poll_fn keeps its captures until the executor drops the future,
        // so the reference count below verifies cleanup after catching the panic.
        spawn_local(poll_fn(move |cx| -> Poll<()> {
            task_polls.set(task_polls.get() + 1);
            *task_waker.borrow_mut() = Some(cx.waker().clone());
            if self_wake {
                cx.waker().wake_by_ref();
            }
            panic!("expected detached task panic");
        }));

        sleep(Duration::ZERO).await;
        assert_eq!(polls.get(), 1);
        assert_eq!(Rc::strong_count(&polls), 1, "panicked future was retained");

        let waker = retained_waker.borrow_mut().take().unwrap();
        waker.wake_by_ref();
        waker.wake();

        let ran = Rc::new(Cell::new(false));
        let task_ran = ran.clone();
        spawn_local(async move { task_ran.set(true) });
        sleep(Duration::ZERO).await;
        assert_eq!(polls.get(), 1, "panicked future was polled again");
        assert!(ran.get(), "executor stopped after a task panicked");
    }
}

#[crate::test]
async fn remote_handle_propagates_task_panic_to_awaiter() {
    let state = Rc::new(());
    let task_state = state.clone();
    let (task, handle) = poll_fn(move |_| -> Poll<()> {
        let _ = &task_state;
        panic!("expected remote task panic");
    })
    .remote_handle();
    spawn_local(task);

    // Bound the wait so a missing panic notification fails instead of hanging.
    sleep(Duration::ZERO).await;
    assert_eq!(Rc::strong_count(&state), 1, "panicked future was retained");
    let panic = AssertUnwindSafe(handle)
        .catch_unwind()
        .now_or_never()
        .expect("remote handle did not receive the panic")
        .expect_err("remote handle did not resume the panic");
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"expected remote task panic")
    );
}

#[crate::test]
async fn catches_future_drop_panic_after_completion() {
    struct PanicOnDrop {
        drops: Rc<Cell<usize>>,
        waker: Rc<RefCell<Option<Waker>>>,
    }

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
            // A destructor must not enqueue another poll of the dying task.
            self.waker.borrow().as_ref().unwrap().wake_by_ref();
            panic!("expected future destructor panic");
        }
    }

    let drops = Rc::new(Cell::new(0));
    let polls = Rc::new(Cell::new(0));
    let task_polls = polls.clone();
    let waker = Rc::new(RefCell::new(None));
    let on_drop = PanicOnDrop {
        drops: drops.clone(),
        waker: waker.clone(),
    };
    // Keep the destructor in the future until the executor drops it.
    spawn_local(poll_fn(move |cx| {
        *on_drop.waker.borrow_mut() = Some(cx.waker().clone());
        task_polls.set(task_polls.get() + 1);
        Poll::Ready(())
    }));

    sleep(Duration::ZERO).await;
    assert_eq!(drops.get(), 1);
    assert_eq!(Rc::strong_count(&drops), 1, "future fields were retained");
    assert_eq!(Rc::strong_count(&polls), 1, "future captures were retained");

    let waker = waker.borrow_mut().take().unwrap();
    waker.wake_by_ref();
    waker.wake();
    let ran = Rc::new(Cell::new(false));
    let task_ran = ran.clone();
    spawn_local(async move { task_ran.set(true) });
    sleep(Duration::ZERO).await;
    assert_eq!(polls.get(), 1, "destroyed future was polled again");
    assert!(ran.get(), "executor stopped after a destructor panicked");
}

#[crate::test]
async fn spawn_microtasks() {
    let mut ticks = crate::task::timeout_loop(Duration::from_millis(10));
    let (broadcaster_tx, broadcaster_rx) = tokio::sync::broadcast::channel(1);
    let (done_tx, done_rx) = oneshot::channel();
    spawn_local(async move {
        for _ in 0..100 {
            ticks.recv().await;
            broadcaster_tx.send(()).unwrap();
        }
        done_tx.send(()).unwrap();
    });

    for _ in 0..100000 {
        let mut rx = broadcaster_rx.resubscribe();
        spawn_local(async move {
            while let Ok(_) = rx.recv().await {
            }
        });
    }

    done_rx.await.unwrap()
}
