use super::{LocalPool, PromiseWaker, block_on};
use crate::task::sleep;
use futures::{
    FutureExt,
    channel::oneshot,
    future::{Either, join, join_all, pending, poll_fn, select, try_join},
    task::LocalSpawnExt,
};
use std::{
    cell::Cell,
    rc::Rc,
    sync::Arc,
    task::{Poll, Waker},
    time::Duration,
};

#[test]
fn pending_wakes_are_coalesced_and_consumed() {
    let notify = Arc::new(PromiseWaker::default());
    let waker = Waker::from(notify.clone());
    assert!(!notify.woken());

    for _ in 0..3 {
        waker.wake_by_ref();
        waker.clone().wake();
        assert!(notify.woken());

        notify.wait();
        assert!(!notify.woken());
    }
}

#[test]
fn self_wakes_and_timer_wakes_can_alternate() {
    block_on(async {
        for _ in 0..3 {
            let mut polls = 0;
            poll_fn(|cx| {
                polls += 1;
                if polls < 16 {
                    cx.waker().wake_by_ref();
                    cx.waker().clone().wake();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
            assert_eq!(polls, 16);
            sleep(Duration::ZERO).await;
        }
    });
}

#[test]
fn local_pool_stalls_after_consuming_self_wakes_and_can_resume() {
    let mut pool = LocalPool::new();
    let polls = Rc::new(Cell::new(0));
    let task_polls = polls.clone();
    let (sender, receiver) = oneshot::channel::<()>();

    pool.spawner()
        .spawn_local(async move {
            poll_fn(|cx| {
                task_polls.set(task_polls.get() + 1);
                if task_polls.get() < 3 {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
            receiver.await.unwrap();
        })
        .unwrap();

    pool.run_until_stalled();
    assert_eq!(polls.get(), 3);
    assert!(!pool.try_run_one());
    sender.send(()).unwrap();
    assert!(pool.try_run_one());
    assert!(!pool.try_run_one());
}

async fn after_yields(count: u8) -> u8 {
    for _ in 0..count {
        sleep(Duration::ZERO).await;
    }
    count
}

#[test]
fn join_drives_both_branches_and_handles_repeated_wakeups() {
    let (request_tx, request_rx) = oneshot::channel();
    let (reply_tx, reply_rx) = oneshot::channel();

    let result = block_on(join(
        async {
            let request = request_rx.await.unwrap();
            sleep(Duration::ZERO).await;
            reply_tx.send(request + 1).unwrap();
            request
        },
        async {
            sleep(Duration::ZERO).await;
            request_tx.send(41).unwrap();
            reply_rx.await.unwrap()
        },
    ));

    assert_eq!(result, (41, 42));
}

#[test]
fn join_macro_waits_for_all_branches() {
    let result =
        block_on(async { futures::join!(after_yields(3), after_yields(1), after_yields(2)) });

    assert_eq!(result, (3, 1, 2));
}

#[test]
fn join_all_preserves_input_order_when_completions_are_reversed() {
    let (senders, receivers): (Vec<_>, Vec<_>) =
        (0..40).map(|_| oneshot::channel::<usize>()).unzip();

    let (results, ()) = block_on(join(join_all(receivers), async {
        for (value, sender) in senders.into_iter().enumerate().rev() {
            sleep(Duration::ZERO).await;
            sender.send(value).unwrap();
        }
    }));

    assert_eq!(results, (0..40).map(Ok).collect::<Vec<_>>());
}

#[test]
fn select_handles_either_winner_and_resumes_the_loser() {
    for left_wins in [true, false] {
        let mut pool = LocalPool::new();
        let (left_tx, left_rx) = oneshot::channel();
        let (right_tx, right_rx) = oneshot::channel();
        let (continue_tx, continue_rx) = oneshot::channel();

        pool.spawner()
            .spawn_local(async move {
                let (first, second) = if left_wins {
                    (left_tx, right_tx)
                } else {
                    (right_tx, left_tx)
                };
                sleep(Duration::ZERO).await;
                first.send(10).unwrap();
                // The loser stays pending until select has returned its winner.
                continue_rx.await.unwrap();
                sleep(Duration::ZERO).await;
                second.send(20).unwrap();
            })
            .unwrap();

        pool.run_until(async {
            let remaining = match select(left_rx, right_rx).await {
                Either::Left((value, remaining)) => {
                    assert!(left_wins);
                    assert_eq!(value.unwrap(), 10);
                    remaining
                }
                Either::Right((value, remaining)) => {
                    assert!(!left_wins);
                    assert_eq!(value.unwrap(), 10);
                    remaining
                }
            };
            continue_tx.send(()).unwrap();
            assert_eq!(remaining.await.unwrap(), 20);
        });
    }
}

#[test]
fn select_macro_completes_with_another_branch_still_pending() {
    let result = block_on(async {
        let completed = after_yields(2).fuse();
        let never = pending::<u8>().fuse();
        futures::pin_mut!(completed, never);

        futures::select! {
            value = completed => value,
            _ = never => panic!("pending future completed"),
        }
    });

    assert_eq!(result, 2);
}

#[test]
fn try_join_returns_error_and_drops_the_pending_branch() {
    let (sender, receiver) = oneshot::channel::<Result<u8, &str>>();
    let result = block_on(try_join(async { receiver.await.unwrap() }, async {
        sleep(Duration::ZERO).await;
        Err::<u8, _>("failed")
    }));

    assert_eq!(result, Err("failed"));
    assert!(sender.is_canceled());
}

#[test]
fn local_pool_runs_spawned_join_to_completion() {
    let mut pool = LocalPool::new();
    let result = Rc::new(Cell::new(None));
    let task_result = result.clone();

    pool.spawner()
        .spawn_local(async move {
            task_result.set(Some(join(after_yields(2), after_yields(1)).await));
        })
        .unwrap();
    pool.run();

    assert_eq!(result.get(), Some((2, 1)));
}
