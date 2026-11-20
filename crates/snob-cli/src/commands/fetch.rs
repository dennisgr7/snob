//! `snob fetch`: one file from Instagram's CDN, by an address already known
//! (`snob_engine::fetch`, which holds it to the CDN and names the file).

use std::path::Path;

use anyhow::Result;
use snob_engine::exit::ExitCode;
use snob_engine::fetch;

use crate::cli::FetchArgs;
use crate::output;
use crate::ui;

/// Fetches `args.url`, sending `--user-agent`, else the one `account_ua` reads
/// off the account, else the installed browser's.
pub async fn run(
    args: FetchArgs,
    account_ua: impl FnOnce() -> Result<Option<String>>,
) -> Result<ExitCode> {
    let url = fetch::checked(&args.url)?;
    let to_stdout_asked = args
        .output
        .as_deref()
        .is_some_and(snob_engine::files::is_stdout);
    if to_stdout_asked {
        snob_engine::files::one_file_to_stdout(1)?;
    }
    if let Some(dir) = &args.into
        && !dir.is_dir()
    {
        anyhow::bail!("{} is not a directory", dir.display());
    }
    let user_agent = fetch::user_agent(args.user_agent.clone(), account_ua)?;
    let cdn = fetch::client(&user_agent, &url)?;

    match (args.output.as_deref(), args.into.as_deref()) {
        (_, Some(dir)) => {
            let path = fetch::into_dir(&cdn, &url, dir).await?;
            if args.json {
                println!("{}", serde_json::json!({ "saved": path }));
            }
            ui::info(&format!("Saved {}", path.display()));
        }
        (Some(_), None) if to_stdout_asked => fetch::to_stdout(&cdn, &url).await?,
        (Some(path), None) => {
            fetch::to_path(&cdn, &url, path).await?;
            ui::info(&format!("Saved {}", path.display()));
        }
        (None, None) if output::Presentation::detect(None).interactive => {
            let path = fetch::into_dir(&cdn, &url, Path::new(".")).await?;
            ui::info(&format!("Saved {}", path.display()));
        }
        (None, None) => fetch::to_stdout(&cdn, &url).await?,
    }
    Ok(ExitCode::Ok)
}
