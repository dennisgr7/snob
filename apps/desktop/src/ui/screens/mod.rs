//! The screens: one function each, from what the shell knows to what is
//! drawn in the panel. **Every screen has one title**, a heading of the
//! first level that focus goes to when the screen comes in, and says every
//! state in words as well as in shape.

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Div, Window, div, px};
use snob_core::Epoch;
use snob_engine::login::Failure;

use super::frame::clock;
use super::look::{Look, sp};
use super::shell::{Phase, Shell};
use crate::state::flow::{Method, Named, Place, Screen};

mod failed;
mod home;
mod paste;
mod splash;
mod start;
mod waiting;
mod welcome;

/// An account as a sentence names it: its username with the at sign, or
/// its number while no name is known.
pub fn handle(account: &Named) -> String {
    match &account.username {
        Some(username) => format!("@{username}"),
        None => account.pk.to_string(),
    }
}

/// What a section is called: its title, and its button's name on the rail.
pub fn section_title(place: Place, look: &Look) -> String {
    match place {
        Place::Home => look.tr("home-title"),
        Place::Search => look.tr("search-title"),
        Place::Automations => look.tr("automations-title"),
    }
}

/// What a failed sign-in is said as: what happened and what to do, with no
/// jargon and no code.
pub fn failure(why: Failure, retry: Method, until: Option<Epoch>, look: &Look) -> String {
    match why {
        Failure::Network => look.tr("error-offline"),
        Failure::Rejected if retry == Method::Paste => look.tr("error-rejected"),
        Failure::Rejected => look.tr("error-rejected-browser"),
        Failure::Challenge => look.tr("error-challenge"),
        Failure::PushedBack => match until {
            Some(until) => look.tr_with("error-paused-until", &[("time", &clock(until))]),
            None => look.tr("error-paused"),
        },
        Failure::NoBrowser => look.tr("error-no-browser"),
        Failure::TimedOut => look.tr("error-timed-out"),
        Failure::Malformed => look.tr("error-malformed"),
        Failure::WindowClosed | Failure::Other => look.tr("error-unexpected"),
    }
}

/// What a screen's parts are laid in: a column in the middle of the panel,
/// at the place its coming or going has it.
fn column(phase: Phase) -> Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(sp(18.))
        .px(sp(32.))
        .py(sp(40.))
        .relative()
        .top(px(phase.rise))
        .opacity(phase.presence)
}

/// The screen the shell shows.
pub fn render(
    shell: &mut Shell,
    phase: Phase,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    match shell.shown().clone() {
        Screen::Splash => splash::render(phase, cx),
        Screen::Start { expired, back } => start::render(shell, expired, back, phase, cx),
        Screen::Updating { at, .. } => waiting::updating(shell, at, phase, window, cx),
        Screen::Browser => waiting::browser(shell, phase, window, cx),
        Screen::Paste => paste::render(shell, phase, window, cx),
        Screen::Failed { why, id, retry } => failed::render(shell, why, id, retry, phase, cx),
        Screen::Welcome(account) => welcome::render(shell, account, phase, cx),
        Screen::Home => home::home(shell, phase, cx),
        Screen::Search => home::search(shell, phase, cx),
        Screen::Automations => home::automations(shell, phase, cx),
        Screen::Settings => home::settings(shell, phase, cx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Settings;

    #[test]
    fn every_failure_says_what_happened_and_what_to_do() {
        let look = Look::of(Settings::default(), false);
        let all = [
            Failure::NoBrowser,
            Failure::WindowClosed,
            Failure::TimedOut,
            Failure::Malformed,
            Failure::Rejected,
            Failure::Challenge,
            Failure::PushedBack,
            Failure::Network,
            Failure::Other,
        ];
        for why in all {
            for retry in [Method::Browser, Method::Paste] {
                let said = failure(why, retry, None, &look);
                assert!(said.ends_with('.'), "{why:?}: {said}");
                assert!(!said.starts_with("error-"), "{why:?} has no words");
            }
        }
        // A session Instagram refuses is not the same advice for a paste.
        assert_ne!(
            failure(Failure::Rejected, Method::Paste, None, &look),
            failure(Failure::Rejected, Method::Browser, None, &look)
        );
        let until = Epoch::new(1_800_000_000);
        assert!(
            failure(Failure::PushedBack, Method::Paste, Some(until), &look).contains(&clock(until))
        );
    }

    #[test]
    fn an_account_is_named_by_its_username_or_its_number() {
        let named = |username: Option<&str>| Named {
            pk: 42.into(),
            username: username.map(str::to_string),
        };
        assert_eq!(handle(&named(Some("me"))), "@me");
        assert_eq!(handle(&named(None)), "42");
    }
}
