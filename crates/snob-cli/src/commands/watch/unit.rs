//! `snob watch unit`: what to give a scheduler, printed and never installed.
//!
//! **Every unit runs `snob watch once --when-due` every quarter of an hour**,
//! whatever the schedule is, and the run decides whether it is due
//! (`once::is_due`, the evaluator the scheduled mode sleeps on).

use anyhow::Result;
use snob_engine::exit::ExitCode;

use crate::cli::{UnitKind, WatchUnitArgs};
use crate::ui;

/// How often every unit asks. A quarter of an hour is how late a run on a
/// calendar may start, against 96 starts a day that send nothing.
const ASK_EVERY_MINUTES: u32 = 15;

/// What every unit runs, after the binary.
const RUN: [&str; 3] = ["watch", "once", "--when-due"];

pub(super) fn unit(args: WatchUnitArgs, paths: &snob_store::paths::AppPaths) -> Result<ExitCode> {
    let exe = snob_engine::system::exe::to_print(
        "a scheduler started from it stops working once an upgrade removes that directory; \
         print the unit again after an upgrade",
    )?;
    let exe = exe.to_string_lossy();
    fits_a_unit(&exe)?;
    let kind = args.kind.unwrap_or_else(UnitKind::of_this_platform);
    ui::say!("{}", render(kind, &exe).trim_end());
    ui::info(
        "The schedule stays in watch.toml: each run asks whether it is due, and one that is \
         not sends nothing. Add --account <name> before \"watch\" if this computer signs in \
         more than one account.",
    );
    // A file that cannot be read is `watch check`'s to explain; this only
    // warns when it can tell.
    let configured = snob_store::config::load(paths).ok().flatten();
    if configured.is_some_and(|config| config.webhook.is_none()) {
        ui::warn(NO_WEBHOOK);
    }
    Ok(ExitCode::Ok)
}

/// Said when `watch.toml` names no webhook.
const NO_WEBHOOK: &str = "watch.toml names no [webhook], so the reports these runs make are \
     printed where the scheduler keeps a run's output, if it keeps any, and \"snob watch diff\" \
     no longer shows them once a run has made them. Set one with \"snob watch setup\".";

/// Refuses a path no scheduler's file can hold: a crontab line ends at a line
/// break, and in a unit file what follows one is read as a directive of its
/// own.
fn fits_a_unit(exe: &str) -> Result<()> {
    if exe.chars().any(char::is_control) {
        anyhow::bail!(
            "the path to snob holds a control character, which no scheduler's file can \
             carry; install snob at a path without one"
        );
    }
    Ok(())
}

/// The unit for `kind`, running `exe`.
fn render(kind: UnitKind, exe: &str) -> String {
    match kind {
        UnitKind::Systemd => systemd(exe),
        UnitKind::Cron => cron(exe),
        UnitKind::Launchd => launchd(exe),
        UnitKind::Schtasks => schtasks(exe),
    }
}

/// A user service and its timer.
fn systemd(exe: &str) -> String {
    // systemd reads `%` as a specifier, `$` as a variable, quoted or not,
    // and splits on spaces outside quotes.
    let quoted = format!(
        "\"{}\"",
        exe.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    );
    format!(
        "# ~/.config/systemd/user/snob-watch.service\n\
         [Unit]\n\
         Description=snob monitor, when watch.toml says a run is due\n\
         Wants=network-online.target\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={quoted} {run}\n\
         SuccessExitStatus=5 130\n\
         Nice=10\n\
         \n\
         # ~/.config/systemd/user/snob-watch.timer\n\
         [Unit]\n\
         Description=Ask snob every {ASK_EVERY_MINUTES} minutes whether a run is due\n\
         \n\
         [Timer]\n\
         OnCalendar=*:0/{ASK_EVERY_MINUTES}\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n\
         \n\
         # Then: systemctl --user daemon-reload\n\
         #       systemctl --user enable --now snob-watch.timer\n\
         # and, to run while you are logged out: loginctl enable-linger\n\
         # Logged out, the keyring is usually locked: a session saved with\n\
         # \"snob login --no-keyring\" works there; the webhook's token and\n\
         # signing key live only in the keyring.\n",
        run = RUN.join(" "),
    )
}

/// One crontab line. cron reads `%` as a line break, and runs the line in
/// `sh`, so the path is single-quoted.
///
/// **A `%` goes outside the quotes, as `\%`**: cronie takes the backslash off
/// before `sh` sees the line and Vixie cron, Debian's, leaves it, where inside
/// single quotes it would stay in the path. Outside them `sh` reads `%` and
/// `\%` alike.
fn cron(exe: &str) -> String {
    let quoted = format!("'{}'", exe.replace('\'', r"'\''").replace('%', r"'\%'"));
    format!(
        "# crontab -e, and add:\n\
         */{ASK_EVERY_MINUTES} * * * * {quoted} {run}\n\
         # cron starts it with no session bus, where the system keyring may be out of\n\
         # reach: a session saved with \"snob login --no-keyring\" works there; the\n\
         # webhook's token and signing key live only in the keyring.\n",
        run = RUN.join(" "),
    )
}

/// A launch agent: started in the user's session, so the Keychain is open.
fn launchd(exe: &str) -> String {
    let xml = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let arguments: String = std::iter::once(exe)
        .chain(RUN)
        .map(|argument| format!("        <string>{}</string>\n", xml(argument)))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!-- ~/Library/LaunchAgents/dev.dennisgr7.snob.watch.plist\n     \
         Then: launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.dennisgr7.snob.watch.plist -->\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n    \
         <key>Label</key>\n    \
         <string>dev.dennisgr7.snob.watch</string>\n    \
         <key>ProgramArguments</key>\n    \
         <array>\n\
         {arguments}    \
         </array>\n    \
         <key>StartInterval</key>\n    \
         <integer>{seconds}</integer>\n    \
         <key>ProcessType</key>\n    \
         <string>Background</string>\n    \
         <key>LowPriorityIO</key>\n    \
         <true/>\n\
         </dict>\n\
         </plist>\n",
        seconds = ASK_EVERY_MINUTES * 60,
    )
}

/// A scheduled task, registered by the user who pastes it, while signed in: the
/// keyring snob uses there is this user's Credential Manager.
///
/// **Written for PowerShell**, the console Windows opens by default, with the
/// Task Scheduler's own cmdlets, which are the same in Windows PowerShell 5.1
/// and in PowerShell 7.
fn schtasks(exe: &str) -> String {
    // PowerShell ends a single-quoted string at any of five quotes, the four
    // typographic ones included (its `IsSingleQuote`), and takes each doubled
    // as itself: a path through `O’Brien` would otherwise end the string.
    let quoted = |text: &str| {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('\'');
        for c in text.chars() {
            out.push(c);
            if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                out.push(c);
            }
        }
        out.push('\'');
        out
    };
    format!(
        "# Run once, in PowerShell, signed in as the user snob signs in as:\n\
         $action = New-ScheduledTaskAction -Execute {exe} -Argument {run}\n\
         $trigger = New-ScheduledTaskTrigger -Once -At (Get-Date) \
         -RepetitionInterval (New-TimeSpan -Minutes {ASK_EVERY_MINUTES})\n\
         $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries \
         -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew\n\
         Register-ScheduledTask -TaskName 'snob watch' -Action $action -Trigger $trigger \
         -Settings $settings -Force\n",
        exe = quoted(exe),
        run = quoted(&RUN.join(" ")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/opt/snob 100%/bin/snob";
    const DOLLAR: &str = "/opt/$HOME/snob";

    #[test]
    fn every_unit_runs_the_same_command_on_the_same_cadence() {
        for kind in [
            UnitKind::Systemd,
            UnitKind::Cron,
            UnitKind::Launchd,
            UnitKind::Schtasks,
        ] {
            let unit = render(kind, "/usr/bin/snob");
            assert!(unit.contains("/usr/bin/snob"), "{unit}");
            assert!(
                unit.contains("watch once --when-due")
                    || unit.contains("<string>--when-due</string>"),
                "{unit}"
            );
            assert!(unit.contains("15") || unit.contains("900"), "{unit}");
        }
    }

    /// A `%` in the path is a specifier to systemd and a line break to cron.
    #[test]
    fn the_path_is_quoted_as_each_scheduler_reads_it() {
        assert!(systemd(EXE).contains("ExecStart=\"/opt/snob 100%%/bin/snob\" watch once"));
        assert!(systemd(DOLLAR).contains("ExecStart=\"/opt/$$HOME/snob\" watch once"));
        assert!(cron(EXE).contains("'/opt/snob 100'\\%'/bin/snob' watch once"));
        assert!(cron("/opt/o'brien/snob").contains(r"'/opt/o'\''brien/snob' watch once"));
        assert!(launchd("/a&b/snob").contains("<string>/a&amp;b/snob</string>"));
        assert!(
            schtasks(r"C:\Users\o'brien\snob.exe")
                .contains(r"-Execute 'C:\Users\o''brien\snob.exe'")
        );
    }

    /// A line break in the path would start a line of the unit's own.
    #[test]
    fn a_path_with_a_line_break_is_refused_before_any_unit_is_printed() {
        assert!(fits_a_unit("/opt/snob\nExecStartPre=/tmp/other").is_err());
        assert!(fits_a_unit("/opt/snob\r* * * * * other").is_err());
        assert!(fits_a_unit(EXE).is_ok());
        assert!(fits_a_unit(r"C:\Program Files\snob\snob.exe").is_ok());
    }

    /// PowerShell closes a single-quoted string at a typographic quote too,
    /// so each of the five is doubled, or the rest of the path is read as
    /// commands.
    #[test]
    fn a_typographic_quote_in_the_path_does_not_end_the_powershell_string() {
        for quote in ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'] {
            let task = schtasks(&format!(r"C:\Users\o{quote}; calc; {quote}\snob.exe"));
            assert!(
                task.contains(&format!(
                    r"-Execute 'C:\Users\o{quote}{quote}; calc; {quote}{quote}\snob.exe' -Argument"
                )),
                "{task}"
            );
        }
    }

    /// The task is one PowerShell reads as written: the path whole, spaces
    /// and all, in single quotes, where a backslash is a backslash, and the
    /// arguments as one string beside it.
    #[test]
    fn the_windows_task_is_one_powershell_takes_with_a_path_that_has_spaces() {
        let task = schtasks(r"C:\Program Files\snob\snob.exe");
        assert!(
            task.contains(
                r"New-ScheduledTaskAction -Execute 'C:\Program Files\snob\snob.exe' -Argument 'watch once --when-due'"
            ),
            "{task}"
        );
        assert!(task.contains("New-TimeSpan -Minutes 15"), "{task}");
        assert!(
            task.contains("Register-ScheduledTask -TaskName 'snob watch'"),
            "{task}"
        );
        // Nothing only cmd.exe reads: no `rem`, no backslash before a quote,
        // and every line that is not a command is a PowerShell comment.
        assert!(!task.contains("rem "), "{task}");
        assert!(!task.contains("\\\""), "{task}");
        assert!(!task.contains("schtasks"), "{task}");
        for line in task.lines() {
            assert!(
                line.starts_with('#')
                    || line.starts_with('$')
                    || line.starts_with("Register-ScheduledTask"),
                "{line}"
            );
        }
    }

    /// Where the keyring cannot be reached, the notes say what still works
    /// and what is left out.
    #[test]
    fn the_notes_say_where_the_keyring_is_out_of_reach() {
        assert!(systemd("/usr/bin/snob").contains("--no-keyring"));
        assert!(systemd("/usr/bin/snob").contains("signing key"));
        assert!(cron("/usr/bin/snob").contains("signing key"));
        assert!(NO_WEBHOOK.contains("[webhook]"));
    }

    /// A laptop's task: the scheduler's defaults would keep it from starting
    /// on battery and stop it when the charger is pulled.
    #[test]
    fn the_windows_task_may_start_and_go_on_running_on_battery() {
        let task = schtasks(r"C:\snob\snob.exe");
        assert!(task.contains("-AllowStartIfOnBatteries"), "{task}");
        assert!(task.contains("-DontStopIfGoingOnBatteries"), "{task}");
        assert!(task.contains("-MultipleInstances IgnoreNew"), "{task}");
        assert!(task.contains("-Settings $settings"), "{task}");
    }
}
