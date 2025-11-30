use acton::prelude::*;
use crate::messages::InitProject;
use tracing::*;

#[acton_actor]
struct MyActor;

impl MyActor {
pub async fn new(runtime: &mut ActorRuntime) -> ActorHandle {
let mut actor = runtime.new_actor::
<MyActor>().await;
    actor.mutate_on::
    <MyMessage>(|actor, context| {
        info!("MyMessage received by MyActor.");

        ActorReply::immediate()
        });

        actor.start().await
        }
        }
