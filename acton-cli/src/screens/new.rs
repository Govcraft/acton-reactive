use std::fmt::Display;
use std::io::{stdout, Write};

use acton_reactive::prelude::*;
use ansi_term::Color::RGB;
use ansi_term::Style;
use crossterm::{cursor, queue};
use tracing::trace;

use crate::{PADLEFT, PADTOP, TITLE};
use crate::messages::{MenuMoveDown, MenuMoveUp};

#[acton_actor]
pub struct NewScreen {
    menu: Vec<MenuItem>,
}

#[derive(Clone)]
struct MenuItem {
    label: String,
    selected: bool,
}

impl Display for MenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let selected = Style::new().fg(RGB(255,141,204));
        let selected_text = Style::new().fg(RGB(253,209,234));
        let normal = Style::new().fg(RGB(96,96,96));
        if self.selected {
            write!(f, "{} {:<2}", selected.paint(">"), format!("{}", selected_text.paint(self.label.clone())))
        } else {
            write!(f, "  {:<2}", format!("{}", normal.paint(self.label.clone())))
        }
    }
}

impl NewScreen {
    pub async fn create(runtime: &mut ActorRuntime) -> anyhow::Result<ActorHandle> {
        let mut actor = runtime.new_actor::<Self>();

        actor.before_start(|_actor| {
            let mut stdout = stdout();
            queue!(stdout, cursor::MoveTo(PADLEFT, PADTOP)).unwrap();
            let _ = stdout.write(TITLE.as_ref());
            queue!(stdout, cursor::MoveTo(PADLEFT, PADTOP + 1)).unwrap();
            let _ = stdout.write("Scaffold a new application".as_ref());
            let _ = stdout.flush();
            ActorReply::immediate()
        })
            .mutate_on::<MenuMoveDown>(|actor, _context| {
                trace!(" MenuMoveDown");
                let len = actor.model.menu.len();
                if let Some(current_index) = actor.model.menu.iter().position(|item| item.selected) {
                    actor.model.menu[current_index].selected = false;
                    let next_index = (current_index + 1) % len;
                    actor.model.menu[next_index].selected = true;
                }
                actor.model.paint();
                ActorReply::immediate()
            })
            .mutate_on::<MenuMoveUp>(|actor, _context| {
                trace!(" MenuMoveUp");
                let len = actor.model.menu.len();
                if let Some(current_index) = actor.model.menu.iter().position(|item| item.selected) {
                    actor.model.menu[current_index].selected = false;
                    let prev_index = if current_index == 0 { len - 1 } else { current_index - 1 };
                    actor.model.menu.iter_mut().rev().nth(len - prev_index - 1).unwrap().selected = true;
                }
                actor.model.paint();
                ActorReply::immediate()
            })
            .after_start(|actor| {
                actor.model.paint();
                ActorReply::immediate()
            });
        actor.model.menu.push(MenuItem { label: "Name".to_string(), selected: true });
        actor.model.menu.push(MenuItem { label: "Location".to_string(), selected: false });

        Ok(actor.start().await)
    }

    fn paint(&self) {
        let mut stdout = stdout();
        queue!(stdout, cursor::MoveTo(PADLEFT, PADTOP + 3)).unwrap();
        // use crossterm to output the list of menuitems
        for (i, item) in self.menu.iter().enumerate() {
            let row = u16::try_from(i).unwrap_or(u16::MAX);
            queue!(stdout, cursor::MoveTo(PADLEFT, PADTOP + 3 + row)).unwrap();
            let _ = stdout.write(item.to_string().as_ref());
        }
        let _ = stdout.flush();
    }
}
