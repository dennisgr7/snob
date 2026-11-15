//! The desktop app: one window on the engine the command line runs, linked
//! in process. **A skeleton**: it shows the accounts snob knows on this
//! computer and sends nothing; no view reads a session or starts a walk yet.
//!
//! Every rule of AGENTS.md holds here through the engine: a view reaches
//! Instagram through an `App` (`snob_engine::app`), never a client of its
//! own.

// No console window behind the app on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod front;

use gpui_kit::*;
use snob_core::model::printable;
use snob_store::paths::AppPaths;
use snob_store::registry::Registry;

/// An account as the first view lists it.
struct Known {
    username: String,
    pk: String,
    active: bool,
}

/// What the first view shows: read once, when the window opens.
struct Home {
    accounts: Result<Vec<Known>, String>,
    /// Whether the `snob` the engine starts for its own processes was found
    /// beside the app.
    snob_found: bool,
    notices: front::Lines,
}

/// The accounts in `accounts.toml`: a file read, no keyring and no network.
fn known_accounts() -> Result<Vec<Known>, String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let registry = Registry::load(&paths).map_err(|e| e.to_string())?;
    Ok(registry
        .accounts
        .iter()
        .map(|account| Known {
            username: printable(&account.username),
            pk: account.pk.to_string(),
            active: registry.active == Some(account.pk),
        })
        .collect())
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let accounts = match &self.accounts {
            Ok(accounts) if accounts.is_empty() => {
                vec!["No account yet. Sign in with \"snob login\" in a terminal.".to_string()]
            }
            Ok(accounts) => accounts
                .iter()
                .map(|account| {
                    let mark = if account.active { " (active)" } else { "" };
                    format!("{}  {}{mark}", account.username, account.pk)
                })
                .collect(),
            Err(why) => vec![format!("The accounts could not be read: {why}")],
        };
        let engine = if self.snob_found {
            "snob found beside the app".to_string()
        } else {
            "no snob beside the app: nothing that starts a browser or a job can run".to_string()
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .size_full()
            .child(div().text_xl().child("snob"))
            .child(engine)
            .child(div().pt_2().child("Accounts"))
            .children(accounts)
            .children(self.notices.all())
    }
}

fn main() {
    let (notices, snob_found) = front::hand_over();
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("snob".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |_, cx| {
            cx.new(|_| Home {
                accounts: known_accounts(),
                snob_found,
                notices,
            })
        })
        .expect("the first window opens");
    });
}
