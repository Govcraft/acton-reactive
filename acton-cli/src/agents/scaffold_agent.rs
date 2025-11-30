use std::collections::HashMap;
use std::fs;
use std::path::Path;
use acton_reactive::prelude::{acton_actor, ActorHandle, ActorReply, ActorRuntime};
use handlebars::Handlebars;
use crate::messages::InitProject;
use tracing::{error, info};

#[acton_actor]
pub struct ScaffoldActor;

impl ScaffoldActor {
    pub async fn create(runtime: &mut ActorRuntime) -> ActorHandle {
        let mut actor = runtime.new_actor::<Self>();
        actor.mutate_on::<InitProject<'_>>(|_actor, context| {
            let project_name = context.message().project_name;
            let base_path = Path::new(project_name);

            // Create the base project directory
            if let Err(e) = fs::create_dir(base_path) {
                error!("Error creating project directory: {}", e);
            }

            // Create subdirectories for actors, messages, and examples
            let subdirectories = ["src/actors", "src/messages", "examples"];
            for sub in &subdirectories {
                let sub_path = base_path.join(sub);
                if let Err(e) = fs::create_dir_all(&sub_path) {
                    error!("Error creating subdirectory {}: {}", sub, e);
                }
            }

            // Initialize Handlebars registry
            let mut handlebars = Handlebars::new();

            // Register templates
            let cargo_toml_template = include_str!("../templates/basic/Cargo.toml.hbs");
            let main_rs_template = include_str!("../templates/basic/main.rs.hbs");
            let actors_mod_template = include_str!("../templates/basic/actors.rs.hbs");
            let my_actor_template = include_str!("../templates/basic/actors/my_actor.rs.hbs");
            let messages_mod_template = include_str!("../templates/basic/messages.rs.hbs");
            let my_message_template = include_str!("../templates/basic/messages/my_message.rs.hbs");

            handlebars.register_template_string("Cargo.toml", cargo_toml_template).unwrap();
            handlebars.register_template_string("main.rs", main_rs_template).unwrap();
            handlebars.register_template_string("actors.rs", actors_mod_template).unwrap();
            handlebars.register_template_string("actors/my_actor.rs", my_actor_template).unwrap();
            handlebars.register_template_string("messages.rs", messages_mod_template).unwrap();
            handlebars.register_template_string("messages/my_message.rs", my_message_template).unwrap();

            // Create the Cargo.toml file
            let cargo_toml_path = base_path.join("Cargo.toml");
            let mut data = HashMap::new();
            data.insert("project_name", project_name);
            let cargo_toml_content = handlebars.render("Cargo.toml", &data).unwrap();
            if let Err(e) = fs::write(&cargo_toml_path, cargo_toml_content) {
                error!("Error writing Cargo.toml: {}", e);
            }

            // Create the main.rs file
            let main_rs_path = base_path.join("src/main.rs");
            let main_rs_content = handlebars.render("main.rs", &data).unwrap();
            if let Err(e) = fs::write(&main_rs_path, main_rs_content) {
                error!("Error writing main.rs: {}", e);
            }

            // Create the actors module files
            let actors_mod_path = base_path.join("src/actors.rs");
            let actors_mod_content = handlebars.render("actors.rs", &data).unwrap();
            if let Err(e) = fs::write(&actors_mod_path, actors_mod_content) {
                error!("Error writing actors/mod.rs: {}", e);
            }

            let my_actor_path = base_path.join("src/actors/my_actor.rs");
            let my_actor_content = handlebars.render("actors/my_actor.rs", &data).unwrap();
            if let Err(e) = fs::write(&my_actor_path, my_actor_content) {
                error!("Error writing actors/my_actor.rs: {}", e);
            }

            // Create the messages module files
            let messages_mod_path = base_path.join("src/messages.rs");
            let messages_mod_content = handlebars.render("messages.rs", &data).unwrap();
            if let Err(e) = fs::write(&messages_mod_path, messages_mod_content) {
                error!("Error writing messages/mod.rs: {}", e);
            }

            let my_message_path = base_path.join("src/messages/my_message.rs");
            let my_message_content = handlebars.render("messages/my_message.rs", &data).unwrap();
            if let Err(e) = fs::write(&my_message_path, my_message_content) {
                error!("Error writing messages/my_message.rs: {}", e);
            }

            info!("Project '{}' created successfully!", project_name);            ActorReply::immediate()
        });

        actor.start().await
    }
}
