//! The real app, started in a directory of its own and driven through its
//! control channel: what a scenario reads of the window and does to it.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};
use snob_desktop::control::{Answer, CONTROL, Node, Request, Said};
use snob_engine::local_socket::{self, Stream};
use snob_store::paths::AppPaths;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, ReadHalf, WriteHalf};

/// How long a scenario waits for the app to get somewhere.
pub const SOON: Duration = Duration::from_secs(30);

/// How the app is started.
#[derive(Clone, Default)]
pub struct Start {
    /// The fake Instagram every client is pointed at.
    pub instagram: Option<String>,
    pub dark: bool,
    pub more_contrast: bool,
    pub reduce_motion: bool,
    pub text_scale: Option<f32>,
    pub locale: Option<String>,
    /// How many browsers the app lists, whatever this machine has: one,
    /// unless a scenario is about choosing among several.
    pub browsers: usize,
}

/// The app, running.
pub struct Driven {
    child: Child,
    pub paths: AppPaths,
    reading: tokio::io::Lines<BufReader<ReadHalf<Stream>>>,
    writing: WriteHalf<Stream>,
    /// When the process was started, and when it first answered.
    pub started: Instant,
    pub answered_after: Duration,
    /// Where a picture is kept after every step, and how many were: what a
    /// failed scenario is replayed from.
    pub film: Option<(PathBuf, usize)>,
}

/// The arguments `start` is said as.
fn arguments(root: &Path, start: &Start) -> Vec<String> {
    let mut args = vec![
        "--test-root".to_string(),
        root.display().to_string(),
        "--simulate-system".to_string(),
        // A picture is of the same browsers, whichever machine takes it.
        "--browsers".to_string(),
        start.browsers.max(1).to_string(),
    ];
    if let Some(instagram) = &start.instagram {
        args.extend(["--instagram".to_string(), instagram.clone()]);
    }
    for (on, name) in [
        (start.dark, "--dark"),
        (start.more_contrast, "--more-contrast"),
        (start.reduce_motion, "--reduce-motion"),
    ] {
        if on {
            args.push(name.to_string());
        }
    }
    if let Some(scale) = start.text_scale {
        args.extend(["--text-scale".to_string(), scale.to_string()]);
    }
    if let Some(locale) = &start.locale {
        args.extend(["--locale".to_string(), locale.clone()]);
    }
    args
}

/// Starts the app `exe` in `root` without waiting for it.
pub fn spawn(exe: &Path, root: &Path, start: &Start) -> anyhow::Result<Child> {
    Command::new(exe)
        .args(arguments(root, start))
        // Nothing of the developer's own run reaches the app under test.
        .env_remove("SNOB_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))
}

impl Driven {
    /// Starts the app in `root` and waits until it answers.
    pub async fn start(exe: &Path, root: &Path, start: &Start) -> anyhow::Result<Self> {
        let started = Instant::now();
        let mut child = spawn(exe, root, start)?;
        let paths = AppPaths::rooted_at(root);
        let stream = loop {
            match local_socket::connect(&paths, &CONTROL).await {
                Ok(stream) => break stream,
                Err(_) if started.elapsed() < SOON => {
                    if let Some(status) = child.try_wait()? {
                        bail!("the app left before it could be driven: {status}");
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(e) => {
                    let _ = child.kill();
                    return Err(e).context("the app never listened for the test");
                }
            }
        };
        let (reading, writing) = tokio::io::split(stream);
        let mut driven = Self {
            child,
            paths,
            reading: BufReader::new(reading).lines(),
            writing,
            started,
            answered_after: Duration::ZERO,
            film: None,
        };
        driven.look().await?;
        driven.answered_after = started.elapsed();
        Ok(driven)
    }

    /// The process the app runs as.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Asks, and answers with the window as it is after: an error when the
    /// app said the request could not be done. A step that did something is
    /// followed by a frame of the film, when one is being kept.
    pub async fn ask(&mut self, request: Request) -> anyhow::Result<Answer> {
        let acted = !matches!(
            request,
            Request::Look | Request::Capture { .. } | Request::Quit
        );
        let answer = self.asked(request).await?;
        if acted && let Some((folder, frames)) = self.film.clone() {
            let frame = folder.join(format!("{frames:03}.png"));
            if self.asked(Request::Capture { to: frame }).await.is_ok() {
                self.film = Some((folder, frames + 1));
            }
        }
        Ok(answer)
    }

    async fn asked(&mut self, request: Request) -> anyhow::Result<Answer> {
        let mut line = serde_json::to_vec(&request)?;
        line.push(b'\n');
        self.writing.write_all(&line).await?;
        self.writing.flush().await?;
        let answered = tokio::time::timeout(SOON, self.reading.next_line())
            .await
            .map_err(|_| anyhow!("the app did not answer {request:?}"))??
            .ok_or_else(|| anyhow!("the app hung up on {request:?}"))?;
        let answer: Answer = serde_json::from_str(&answered)?;
        match &answer.error {
            Some(why) => bail!("{request:?}: {why}"),
            None => Ok(answer),
        }
    }

    pub async fn look(&mut self) -> anyhow::Result<Answer> {
        self.ask(Request::Look).await
    }

    /// Waits until `wanted` holds of the window, and answers with it then.
    pub async fn until(
        &mut self,
        what: &str,
        wanted: impl Fn(&Answer) -> bool,
    ) -> anyhow::Result<Answer> {
        let since = Instant::now();
        loop {
            let seen = self.look().await?;
            if wanted(&seen) {
                return Ok(seen);
            }
            if since.elapsed() > SOON {
                bail!(
                    "never got to: {what}. The screen is \"{}\", the status {:?}, last said {:?}",
                    seen.screen,
                    seen.status,
                    seen.announced
                );
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    /// Waits until the screen shown is `screen`.
    pub async fn until_screen(&mut self, screen: &str) -> anyhow::Result<Answer> {
        self.until(&format!("the {screen} screen"), |seen| {
            seen.screen == screen
        })
        .await
    }

    pub async fn click(&mut self, id: &str) -> anyhow::Result<Answer> {
        self.ask(Request::Click { id: id.to_string() }).await
    }

    pub async fn press(&mut self, key: &str) -> anyhow::Result<Answer> {
        self.ask(Request::Press {
            key: key.to_string(),
        })
        .await
    }

    pub async fn type_in(&mut self, text: &str) -> anyhow::Result<Answer> {
        self.ask(Request::Type {
            text: text.to_string(),
        })
        .await
    }

    pub async fn copy(&mut self, text: &str) -> anyhow::Result<Answer> {
        self.ask(Request::Copy {
            text: text.to_string(),
        })
        .await
    }

    pub async fn say(&mut self, said: Said) -> anyhow::Result<Answer> {
        self.ask(Request::System { say: said }).await
    }

    pub async fn capture(&mut self, to: &Path) -> anyhow::Result<()> {
        if let Some(folder) = to.parent() {
            std::fs::create_dir_all(folder)?;
        }
        self.ask(Request::Capture {
            to: to.to_path_buf(),
        })
        .await
        .map(drop)
    }

    pub async fn resize(&mut self, width: f32, height: f32) -> anyhow::Result<Answer> {
        self.ask(Request::Resize { width, height }).await
    }

    /// Everything the app logged so far, oldest first.
    pub fn log(&self) -> String {
        let folder = snob_engine::log::dir(&self.paths);
        let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
            .collect();
        files.sort();
        files.reverse();
        files
            .iter()
            .filter_map(|file| std::fs::read_to_string(file).ok())
            .collect()
    }

    /// Closes the app and waits for it to be gone.
    pub async fn quit(mut self) -> anyhow::Result<()> {
        // It may hang up before it answers: either is the app closing.
        let _ = self.ask(Request::Quit).await;
        let since = Instant::now();
        while self.child.try_wait()?.is_none() {
            if since.elapsed() > Duration::from_secs(10) {
                self.child.kill()?;
                bail!("the app did not close when asked");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }
}

impl Drop for Driven {
    fn drop(&mut self) {
        // A scenario that failed leaves no app behind.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The control `id` of what was seen.
pub fn node<'a>(seen: &'a Answer, id: &str) -> anyhow::Result<&'a Node> {
    seen.nodes
        .iter()
        .find(|node| node.id == id)
        .ok_or_else(|| anyhow!("nothing is called {id} on the {} screen", seen.screen))
}

/// The name a screen reader is told for `id`.
pub fn label(seen: &Answer, id: &str) -> anyhow::Result<String> {
    Ok(node(seen, id)?.label.clone().unwrap_or_default())
}

/// Fails with `what` unless `holds`.
pub fn expect(holds: bool, what: impl FnOnce() -> String) -> anyhow::Result<()> {
    if holds { Ok(()) } else { bail!(what()) }
}

/// Fails unless `found` is `wanted`.
pub fn expect_eq<T: PartialEq + std::fmt::Debug>(
    found: T,
    wanted: T,
    what: &str,
) -> anyhow::Result<()> {
    expect(found == wanted, || {
        format!("{what}: found {found:?}, expected {wanted:?}")
    })
}
