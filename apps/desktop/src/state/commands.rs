//! What the command palette offers on each screen, and which of it is what
//! somebody typed. **Nothing here is done only from the palette**: every
//! command is something a button on a screen or in the account's menu does.

use snob_core::Pk;

use super::flow::{Place, Screen};

/// Something the palette does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    SignInBrowser,
    SignInPaste,
    /// To a section: what its button on the rail does.
    Go(Place),
    Settings,
    AddAccount,
    SwitchTo(Pk),
    SignOut,
    OpenLogs,
    CopyReport,
    /// The profile of the account in use: what its row in the account's
    /// menu does.
    Profile,
}

/// What can be done from `screen`, in the order it is listed; `others` are
/// the accounts signed in beside the one in use. Nothing on the way to
/// signing in: those screens have one thing to do, and a way back.
pub fn offered(screen: &Screen, others: &[Pk]) -> Vec<Command> {
    if matches!(screen, Screen::Start { .. }) {
        return vec![Command::SignInBrowser, Command::SignInPaste];
    }
    if !screen.signed_in() {
        return Vec::new();
    }
    let mut commands: Vec<Command> = Place::ALL
        .into_iter()
        .filter(|place| place.screen() != *screen)
        .map(Command::Go)
        .collect();
    if *screen != Screen::Settings {
        commands.push(Command::Settings);
    }
    commands.push(Command::AddAccount);
    commands.extend(others.iter().copied().map(Command::SwitchTo));
    commands.extend([Command::SignOut, Command::OpenLogs, Command::CopyReport]);
    commands.push(Command::Profile);
    commands
}

/// Whether `label` is what `typed` asks for: every word typed is somewhere
/// in it, whatever the case. Nothing typed asks for everything.
pub fn matches(label: &str, typed: &str) -> bool {
    let label = label.to_lowercase();
    typed
        .split_whitespace()
        .all(|word| label.contains(&word.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_typed_is_looked_for_whatever_its_case_and_order() {
        assert!(matches("Copy diagnostic report", ""));
        assert!(matches("Copy diagnostic report", "  "));
        assert!(matches("Copy diagnostic report", "REPORT"));
        assert!(matches("Copy diagnostic report", "rep cop"));
        assert!(matches("Cerrar sesión", "SESIÓN")); // i18n-allow
        assert!(!matches("Copy diagnostic report", "copy logs"));
    }

    #[test]
    fn a_screen_offers_what_can_be_done_from_it_and_nothing_on_the_way_in() {
        let other = Pk::new(9001);
        assert_eq!(
            offered(
                &Screen::Start {
                    expired: None,
                    back: false
                },
                &[other]
            ),
            [Command::SignInBrowser, Command::SignInPaste]
        );
        assert_eq!(
            offered(&Screen::Home, &[other]),
            [
                Command::Go(Place::Search),
                Command::Go(Place::Automations),
                Command::Settings,
                Command::AddAccount,
                Command::SwitchTo(other),
                Command::SignOut,
                Command::OpenLogs,
                Command::CopyReport,
                Command::Profile,
            ]
        );
        assert_eq!(
            offered(&Screen::Settings, &[])[..3],
            [
                Command::Go(Place::Home),
                Command::Go(Place::Search),
                Command::Go(Place::Automations)
            ]
        );
        assert!(!offered(&Screen::Settings, &[]).contains(&Command::Settings));
        assert_eq!(offered(&Screen::Search, &[])[0], Command::Go(Place::Home));
        for on_the_way in [Screen::Splash, Screen::Browser, Screen::Paste] {
            assert!(offered(&on_the_way, &[other]).is_empty());
        }
    }
}
