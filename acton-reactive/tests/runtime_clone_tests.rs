//! Runtime handles share registration and shutdown across every clone path.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;

#[acton_actor]
struct LifecycleProbe;

fn observe_stop(actor: &mut ManagedActor<Idle, LifecycleProbe>, stopped: Arc<AtomicUsize>) {
    actor.after_stop(move |_| {
        stopped.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });
}

async fn shutdown(runtime: &mut ActorRuntime) {
    tokio::time::timeout(Duration::from_secs(5), runtime.shutdown_all())
        .await
        .expect("shutdown must complete for actors registered through any runtime clone")
        .expect("shutdown should succeed");
}

#[tokio::test]
async fn original_runtime_stops_actors_registered_through_early_clones() {
    let mut runtime = ActonApp::launch_async().await;
    let mut cloned = runtime.clone();
    let observer = runtime.clone();
    let stopped = Arc::new(AtomicUsize::new(0));

    let mut original_actor = runtime.new_actor::<LifecycleProbe>();
    observe_stop(&mut original_actor, Arc::clone(&stopped));
    original_actor.start().await;

    let mut cloned_actor = cloned.new_actor_with_config::<LifecycleProbe>(ActorConfig::default());
    observe_stop(&mut cloned_actor, Arc::clone(&stopped));
    cloned_actor.start().await;

    assert_eq!(runtime.actor_count(), 2);
    assert_eq!(cloned.actor_count(), 2);
    assert_eq!(observer.actor_count(), 2);
    shutdown(&mut runtime).await;
    assert_eq!(stopped.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn shutdown_from_early_clone_stops_actors_registered_on_original() {
    let mut runtime = ActonApp::launch_async().await;
    let mut cloned = runtime.clone();
    let stopped = Arc::new(AtomicUsize::new(0));
    let mut actor = runtime.new_actor::<LifecycleProbe>();
    observe_stop(&mut actor, Arc::clone(&stopped));
    actor.start().await;

    shutdown(&mut cloned).await;
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn setup_function_and_actor_runtime_clones_register_with_original() {
    let mut runtime = ActonApp::launch_async().await;
    let mut cloned = runtime.clone();
    let stopped = Arc::new(AtomicUsize::new(0));
    let stop_observer = Arc::clone(&stopped);

    cloned
        .spawn_actor::<LifecycleProbe>(move |mut actor| {
            Box::pin(async move {
                let mut actor_runtime = actor.runtime().clone();
                let mut nested_root = actor_runtime.new_actor::<LifecycleProbe>();
                observe_stop(&mut nested_root, Arc::clone(&stop_observer));
                nested_root.start().await;
                observe_stop(&mut actor, stop_observer);
                actor.start().await
            })
        })
        .await
        .expect("setup should create both roots");

    assert_eq!(runtime.actor_count(), 2);
    assert_eq!(cloned.actor_count(), 2);
    shutdown(&mut runtime).await;
    assert_eq!(stopped.load(Ordering::SeqCst), 2);
}
