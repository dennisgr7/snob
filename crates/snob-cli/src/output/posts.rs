//! A post's files where `-d` and `-o` say, and an item handed to the system
//! viewer.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use snob_engine::exit::ExitCode;
use snob_engine::media::{Named, Saved, download_named, save_file, save_file_to};
use snob_engine::posts::{Opened, Post, fetched_item};
use snob_ig::client::IgClient;

use crate::ui;

/// Downloads `files` the way `-d` does: one file and `-o` names it; several,
/// or none named, and they go into a directory (`-o`, or the working one)
/// under their own names, three at a time, past the ones that fail.
pub(crate) async fn download(
    client: std::sync::Arc<IgClient>,
    files: Vec<Named>,
    destination: Option<&Path>,
) -> Result<ExitCode> {
    if destination.is_some_and(snob_engine::files::is_stdout) {
        snob_engine::files::one_file_to_stdout(files.len())?;
        let file = &files[0];
        let url = file
            .url
            .as_deref()
            .ok_or_else(|| anyhow!("{} has no downloadable version", file.what))?;
        let mut sink = snob_engine::files::StdoutSink::default();
        let streamed = client.download_to(url, file.cap, &mut sink).await;
        sink.finish(streamed)?;
        return Ok(ExitCode::Ok);
    }
    if let ([file], Some(path)) = (files.as_slice(), destination)
        && !path.is_dir()
    {
        save_file_to(&client, file, path).await?;
        ui::info(&format!("Saved {}", path.display()));
        return Ok(ExitCode::Ok);
    }
    if files.len() == 1 && destination.is_none() {
        let file = &files[0];
        return match save_file(&client, file, Path::new(".")).await? {
            Saved::Now(path) => {
                ui::info(&format!("Saved {}", path.display()));
                Ok(ExitCode::Ok)
            }
            Saved::Already(path) => {
                ui::info(&format!("Already saved {}", path.display()));
                Ok(ExitCode::Ok)
            }
        };
    }
    download_named(client, files, destination, "files").await
}

/// Hands item `index` of `post` to the system viewer, from `scratch`.
pub async fn open_item(
    client: &IgClient,
    post: &Post,
    index: usize,
    scratch: &Path,
    opened: &mut Opened,
) -> Result<PathBuf> {
    let path = fetched_item(client, post, index, scratch, opened).await?;
    opener::open(&path).map_err(|e| anyhow!("the system viewer would not start: {e}"))?;
    Ok(path)
}
