//! `snob watch check`: the configuration answered for while somebody is still
//! here to read the answer.

use anyhow::Result;
use snob_store::paths::{AccountPaths, AppPaths};
use snob_store::secrets::SecretStore;

use crate::cli::WatchCheckArgs;
use crate::exit::ExitCode;

use super::preflight::{describe_check, preflight};
use super::wire::check_json;

/// Checks the configuration would work, before it runs unattended.
///
/// **It writes nothing and walks no list.**
pub(super) async fn check(
    args: WatchCheckArgs,
    secrets: SecretStore,
    paths: &AppPaths,
    account: Option<&AccountPaths>,
) -> Result<ExitCode> {
    let report = preflight(&args, &secrets, paths, account).await?;

    if args.output.json {
        let viewer = crate::report::acting_as();
        crate::ui::say!(
            "{}",
            serde_json::to_string_pretty(&check_json(&report, viewer.as_ref()))?
        );
    } else {
        for line in describe_check(&report) {
            crate::ui::say!("{line}");
        }
    }

    Ok(report.verdict().exit_code())
}
