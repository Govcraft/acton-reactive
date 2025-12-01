use acton_reactive::prelude::*;
use tracing_subscriber;
mod actors;
mod messages;

use actors::MyActor;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
// Initialize tracing to log to a file
tracing_subscriber::fmt()
.with_max_level(tracing::Level::INFO)
.with_writer(std::fs::File::create("logs/app.log")?)
.init();

let mut app = ActonApp::launch_async().await;

// Example actor setup
let my_actor = app.new_actor::<MyActor>();
    my_actor.start().await;

    // Shut down Acton system
    app.shutdown_all().await?;
    Ok(())
    }
