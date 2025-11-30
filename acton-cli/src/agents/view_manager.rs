use std::collections::HashMap;

use acton_reactive::prelude::*;

use crate::messages::{MenuMoveDown, MenuMoveUp, MenuSelect};
use crate::screens::{HomeScreen, NewScreen};

#[acton_actor]
pub struct ViewManager {
    screens: HashMap<Ern, ActorHandle>,
    active: ActorHandle,
}

impl ViewManager {
    pub async fn create(app_runtime: &mut ActorRuntime) -> anyhow::Result<ActorHandle> {
        let mut actor = app_runtime.new_actor::<Self>();
        let mut runtime = actor.runtime().clone();
        actor
            .mutate_on::<MenuMoveUp>(|actor, context| {
                let view = actor.model.active.clone();
                let msg = context.message().clone();
                ActorReply::from_async(async move {
                    view.send(msg).await;
                })
            })
            .mutate_on::<MenuMoveDown>(|actor, context| {
                let view = actor.model.active.clone();
                let msg = context.message().clone();
                ActorReply::from_async(async move {
                    view.send(msg).await;
                })
            })
            .mutate_on::<MenuSelect>(|actor, context| {
                let view = actor.model.active.clone();
                let msg = context.message().clone();
                ActorReply::from_async(async move {
                    view.send(msg).await;
                })
            });
        let home_screen = HomeScreen::create(&mut runtime).await?;
        actor.model.screens.insert(home_screen.id(), home_screen);

        let new_screen = NewScreen::create(&mut runtime).await?;
        actor.model.screens.insert(new_screen.id(), new_screen);

        // actor.model.active = actor.model.screens[0].clone();

        actor.broker().subscribe::<MenuMoveUp>().await;
        actor.broker().subscribe::<MenuMoveDown>().await;
        Ok(actor.start().await)
    }
}
