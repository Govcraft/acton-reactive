//! Regression coverage for Send futures that are not Sync (issue #25).
use std::{cell::Cell, future::Future, pin::Pin};

use acton_reactive::prelude::*;
use tokio::sync::mpsc;

#[derive(Debug, Default)]
struct State {
    events: Option<mpsc::UnboundedSender<usize>>,
}

#[derive(Debug, Clone)]
struct Mutable(usize);
#[derive(Debug, Clone)]
struct ReadOnly;
#[derive(Debug, Clone)]
struct FallibleMutable;
#[derive(Debug, Clone)]
struct FallibleReadOnly;

// Cell is Send but not Sync. It remains live across the suspension point.
async fn send_only(value: usize) -> usize {
    let cell = Cell::new(value);
    tokio::task::yield_now().await;
    cell.get()
}

type SendReply = Pin<Box<dyn Future<Output = ()> + Send>>;

// Named callbacks must use the v10 Send-only return signature as well.
fn named_handler<T>(actor: &ManagedActor<Started, State>, _: &mut T) -> SendReply {
    let tx = actor.model.events.clone().expect("events configured");
    Reply::pending(async move {
        tx.send(send_only(6).await).expect("receiver remains alive");
    })
}

#[tokio::test]
async fn mutable_send_only_futures_complete_in_mailbox_order() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut actor = runtime.new_actor::<State>();
    actor.mutate_on::<Mutable>(move |_, ctx| {
        let tx = tx.clone();
        let value = ctx.message().0;
        Reply::pending(async move {
            let value = send_only(value).await;
            tx.send(value).expect("receiver remains alive");
        })
    });
    let handle = actor.start().await;
    for value in 0..32 {
        handle.send(Mutable(value)).await;
    }
    for expected in 0..32 {
        let actual = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await?;
        assert_eq!(actual, Some(expected));
    }
    runtime.shutdown_all().await?;
    Ok(())
}

#[tokio::test]
async fn readonly_fallible_and_lifecycle_futures_accept_send_only_work() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut actor = runtime.new_actor::<State>();
    actor.model.events = Some(tx.clone());
    for hook in [0, 1, 2, 3] {
        let tx = tx.clone();
        let callback = move |_: &ManagedActor<Started, State>| {
            let tx = tx.clone();
            async move {
                tx.send(send_only(hook).await)
                    .expect("receiver remains alive");
            }
        };
        match hook {
            0 => {
                actor.before_start(callback);
            }
            1 => {
                actor.after_start(callback);
            }
            2 => {
                actor.before_stop(callback);
            }
            _ => {
                actor.after_stop(callback);
            }
        }
    }
    actor.act_on::<ReadOnly>(named_handler);
    let mutable_tx = tx.clone();
    actor.try_mutate_on::<FallibleMutable, (), std::io::Error>(move |_, _| {
        let tx = mutable_tx.clone();
        Reply::try_pending(async move {
            tx.send(send_only(4).await).expect("receiver remains alive");
            Ok(())
        })
    });
    actor.try_act_on::<FallibleReadOnly, (), std::io::Error>(move |_, _| {
        let tx = tx.clone();
        Reply::try_pending(async move {
            tx.send(send_only(5).await).expect("receiver remains alive");
            Ok(())
        })
    });
    let handle = actor.start().await;
    handle.send(ReadOnly).await;
    handle.send(FallibleMutable).await;
    handle.send(FallibleReadOnly).await;
    let mut seen = Vec::new();
    for _ in 0..5 {
        seen.push(tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await?);
    }
    runtime.shutdown_all().await?;
    for _ in 0..2 {
        seen.push(tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await?);
    }
    seen.sort_unstable();
    assert_eq!(seen, (0..7).map(Some).collect::<Vec<_>>());
    Ok(())
}
