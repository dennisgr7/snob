//! `snob pfp`: saves an account's profile picture at the best size Instagram serves over the web
//! (`snob_engine::profile::picture`).

use anyhow::Result;
use snob_core::model::printable;
use snob_engine::app::{App, Viewer};
use snob_engine::engine::target;
use snob_engine::exit::ExitCode;
pub use snob_engine::profile::{Picture, Source, picture_named as fetch};
use snob_store::paths::AccountPaths;
use snob_store::secrets::SecretStore;

use crate::cli::PfpArgs;
use crate::commands::common::{self, Switched};
use crate::output::{self, Rendered};
use crate::report;
use crate::ui;

pub async fn run(args: PfpArgs, store: SecretStore, paths: &AccountPaths) -> Result<ExitCode> {
    let browses = args.browses(crate::ui::a_human_would_watch_the_listing_scroll_by());
    // `-o -` on a terminal is refused before the profile is read and the
    // picture fetched, not after.
    snob_engine::files::stdout_download_ahead(args.output.as_deref(), Some(1))?;
    if args.json {
        common::refuse_json_to_stdout(args.output.as_deref())?;
    }

    // No bar: three requests do not need one, and the picture goes to standard
    // output when there is no `-o`.
    let app = common::reader(&store, paths, args.interactive, false)?;

    // Nothing here records a cooldown.
    let before = app.client().pacer().spent();
    let known = target::known_pk(&app, &args.target)?;
    let picture = fetch(app.client(), &args.target, known).await?;

    if app.cancel().is_canceled() {
        return Ok(ExitCode::Interrupted);
    }

    // The viewer instead of the download — the default at a terminal, the
    // decision made above.
    if browses {
        return browse(app, picture, before, &store, paths).await;
    }

    // Said before the file is written, so a smaller picture is not a caveat
    // tacked onto "Written to ...".
    announce(&picture.source);
    if args.json {
        return tell(&picture, args.output.as_deref(), args.embed);
    }

    // Worked out before the bytes are moved into the payload: the name comes
    // from what actually arrived, not from the URL.
    let extension = picture.extension();
    let username = picture.username;
    let rendered = Rendered::Bytes(picture.bytes);

    match args.output {
        // `-o -`: standard output, said, and refused on a terminal.
        Some(path) if snob_engine::files::is_stdout(&path) => {
            snob_engine::files::one_file_to_stdout(1)?;
            output::write_rendered(&rendered, None)?;
        }
        // A directory that is there already: the picture goes into it under the
        // name it would get in the working directory, never replacing one.
        Some(dir) if dir.is_dir() => {
            let path = dir.join(snob_engine::files::default_path(
                &dir, &username, extension,
            )?);
            output::write_new(&rendered, &path)?;
        }
        // The user named it, so replacing what is there is their call.
        Some(path) => output::write_rendered(&rendered, Some(&path))?,
        // A JPEG dumped into a terminal is unreadable noise, so on a terminal
        // it gets a name of its own — created exclusively, because that name
        // came off a server rather than out of anybody's keyboard.
        None if output::Presentation::detect(None).interactive => {
            let path =
                snob_engine::files::default_path(std::path::Path::new("."), &username, extension)?;
            output::write_new(&rendered, &path)?;
        }
        None => output::write_rendered(&rendered, None)?,
    }
    Ok(ExitCode::Ok)
}

/// `--json`: what was fetched, as one document on standard output, the picture
/// saved first when `-o` names a directory ([`save_named`]).
fn tell(picture: &Picture, directory: Option<&std::path::Path>, embed: bool) -> Result<ExitCode> {
    let saved = directory.map(|dir| save_named(picture, dir)).transpose()?;
    ui::say!(
        "{}",
        serde_json::to_string_pretty(&document(picture, saved, embed))?
    );
    Ok(ExitCode::Ok)
}

/// The document `--json` prints: `pictures` holds the picture itself only
/// with `embed` ([`snob_engine::media::picture_entry`]).
fn document(picture: &Picture, saved: Option<serde_json::Value>, embed: bool) -> serde_json::Value {
    let (width, height) = match &picture.source {
        Source::FullSize { size: Some((w, h)) } => (Some(*w), Some(*h)),
        _ => (None, None),
    };
    let mut document = serde_json::json!({
        "username": picture.username,
        "full_size": matches!(picture.source, Source::FullSize { .. }),
        "width": width,
        "height": height,
        "bytes": picture.bytes.len(),
        "saved": saved,
    });
    if embed {
        document["pictures"] =
            serde_json::json!([snob_engine::media::picture_entry(1, &picture.bytes)]);
    }
    document
}

/// The picture saved into `dir`, a directory made when it is not there, under
/// `<account>-<digest>` and never over a file that is ([`tell`]): what is
/// said of it is where it is, and `already` when an earlier run saved the same
/// picture, which is not written again.
fn save_named(picture: &Picture, dir: &std::path::Path) -> Result<serde_json::Value> {
    use anyhow::Context as _;

    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let directory = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let stem = format!(
        "{}-{}",
        printable(&picture.username),
        snob_engine::media::short_digest(&picture.bytes)
    );
    let name = format!("{stem}.{}", picture.extension());
    let already = directory.join(&name).is_file();
    if !already {
        let name = snob_engine::files::default_path(&directory, &stem, picture.extension())?;
        output::write_new(
            &Rendered::Bytes(picture.bytes.clone()),
            &directory.join(name),
        )?;
    }
    Ok(serde_json::json!({
        "directory": directory.to_string_lossy(),
        "file": name,
        "already": already,
    }))
}

/// The viewer, as the account the command runs as and then as each account it
/// switches to, fetched again as that one.
async fn browse(
    app: Box<App>,
    picture: Picture,
    before: u32,
    store: &SecretStore,
    paths: &AccountPaths,
) -> Result<ExitCode> {
    struct Session {
        app: Box<App>,
        picture: Picture,
        before: u32,
    }
    common::switching(
        Session {
            app,
            picture,
            before,
        },
        async |s: &mut Session, note: String| {
            crate::ui::pfp::browse(&s.picture, s.app.viewer(), paths, store, note).await
        },
        async |s: &Session, to: Viewer| {
            let name = s.picture.username.clone();
            let from = s.app.viewer();
            let switched = common::switch(store, paths, from, &to, false, async |app: &mut App| {
                let known = target::known_pk(app, &name)?;
                fetch(app.client(), &name, known).await
            })
            .await?;
            Ok(match switched {
                Switched::To((app, picture)) => Switched::To(Session {
                    app,
                    picture,
                    before: 0,
                }),
                Switched::Refused(note) => Switched::Refused(note),
            })
        },
        |s: &Session, several: bool| {
            let spent = s.app.client().pacer().spent().saturating_sub(s.before);
            let whom = if several {
                format!(" as {}", s.app.viewer().label())
            } else {
                String::new()
            };
            ui::info(&format!(
                "profile picture of @{}{whom} - {}",
                printable(&s.picture.username),
                report::requests(spent)
            ));
        },
    )
    .await
}

/// A picture the viewer's tests can hold without a fetch.
#[cfg(test)]
pub(crate) fn for_tests(username: &str, bytes: Vec<u8>) -> Picture {
    Picture {
        username: username.into(),
        url: String::new(),
        bytes,
        source: Source::FullSize { size: None },
    }
}

/// What the viewer's row calls the picture: the size when the by-id
/// endpoint declared one, and an honest word for the fallback.
pub(crate) fn label(source: &Source) -> String {
    match source {
        Source::FullSize { size: Some((w, h)) } => format!("{w}x{h}"),
        Source::FullSize { size: None } => "full size".to_string(),
        Source::Smaller { .. } => "the smaller profile-page size".to_string(),
    }
}

/// What to tell the user before the file is written, if anything.
///
/// Said **before** the write, so it does not read as a caveat attached to
/// "Written to picture.jpg" after the fact.
fn announce(source: &Source) {
    match source {
        Source::FullSize { size: Some((w, h)) } => ui::info(&format!("Full size: {w}x{h}.")),
        Source::FullSize { size: None } => {}
        Source::Smaller { why } => {
            let mut line = "the full-size picture could not be found, so this is the \
                            smaller picture the profile page serves"
                .to_string();
            if let Some(why) = why {
                line.push_str(&format!(" ({why})"));
            }
            ui::warn(&line);
        }
    }
}

#[cfg(test)]
mod tests {
    use snob_core::budget::{RateBudget, RateBudgetError};
    use snob_core::session::{Session, SessionOrigin};
    use snob_ig::client::IgClient;
    use snob_ig::pace::Pacer;
    use url::Url;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    /// A picture the tests can hold without a fetch.
    fn picture_for_tests(username: &str, bytes: Vec<u8>) -> Picture {
        Picture {
            username: username.into(),
            url: String::new(),
            bytes,
            source: Source::FullSize { size: None },
        }
    }

    /// `--json` saves the picture under the account and a digest of it, so a
    /// changed picture is a new file and the same one is not written twice,
    /// and tells where, with the picture itself only when asked.
    #[test]
    fn a_picture_told_as_a_document_is_named_after_its_digest_and_saved_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("kept");
        let first = picture_for_tests("someone", JPEG.to_vec());
        let saved = save_named(&first, &dir).unwrap();
        let name = saved["file"].as_str().unwrap().to_string();
        assert!(
            name.starts_with("someone-") && name.ends_with(".jpg"),
            "{name}"
        );
        assert_eq!(saved["already"], false);
        assert_eq!(std::fs::read(dir.join(&name)).unwrap(), JPEG);

        let again = save_named(&first, &dir).unwrap();
        assert_eq!(
            (again["file"].as_str(), again["already"].as_bool()),
            (Some(name.as_str()), Some(true))
        );

        let mut other = JPEG.to_vec();
        other.push(1);
        let changed = save_named(&picture_for_tests("someone", other), &dir).unwrap();
        assert_ne!(changed["file"], saved["file"]);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);

        let told = document(&first, Some(saved), false);
        assert_eq!(told["username"], "someone");
        assert_eq!(told["bytes"], JPEG.len());
        assert!(told.get("pictures").is_none(), "no picture unless asked");
        let embedded = document(&first, None, true);
        assert_eq!(embedded["saved"], serde_json::Value::Null);
        assert_eq!(embedded["pictures"][0]["mime"], "image/jpeg");
        assert_eq!(embedded["pictures"][0]["of"], 1);
        assert!(embedded["pictures"][0]["data"].is_string());

        // What is no picture, or too large, is a missing entry, not an error.
        let big = picture_for_tests("someone", {
            let mut bytes = JPEG.to_vec();
            bytes.resize(600 * 1024, 0);
            bytes
        });
        assert!(document(&big, None, true)["pictures"][0]["missing"].is_string());
    }

    const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";
    const SID: &str = "42%3AAbCdEfGh%3A20";
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];

    /// The budget is exercised for real elsewhere; here it only has to not get
    /// in the way, and to be counted.
    #[derive(Default)]
    struct CountingBudget(std::sync::atomic::AtomicUsize);

    impl RateBudget for CountingBudget {
        fn reserve(&self) -> Result<std::time::Duration, RateBudgetError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(std::time::Duration::ZERO)
        }
        fn reserve_write(&self) -> Result<std::time::Duration, RateBudgetError> {
            self.reserve()
        }
        fn cooldown(&self) -> Result<Option<snob_core::EpochMs>, RateBudgetError> {
            Ok(None)
        }
        fn start_cooldown(
            &self,
            _reason: &str,
            _minimum: std::time::Duration,
        ) -> Result<snob_core::EpochMs, RateBudgetError> {
            Ok(snob_core::EpochMs::new(0))
        }
    }

    async fn fetch_with(server: &MockServer, target: &str) -> (Result<Picture>, usize) {
        let budget = std::sync::Arc::new(CountingBudget::default());
        let session = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        let client = IgClient::new(session, Pacer::new(budget.clone()))
            .unwrap()
            .with_base_url(Url::parse(&server.uri()).unwrap());

        let result = fetch(&client, target, None).await;
        let reserved = budget.0.load(std::sync::atomic::Ordering::Relaxed);
        (result, reserved)
    }

    /// Mounts the profile lookup plus a picture served from the same mock, so
    /// the absolute CDN URL is reachable from the test.
    async fn profile_serving(server: &MockServer, body: String) {
        Mock::given(method("GET"))
            .and(path("/api/v1/users/web_profile_info/"))
            .and(query_param("username", "someone"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
        for name in ["/full.jpg", "/hd.jpg", "/small.jpg"] {
            Mock::given(method("GET"))
                .and(path(name))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(JPEG.to_vec()))
                .mount(server)
                .await;
        }
    }

    /// The by-id endpoint, which is the only one carrying the full-size
    /// picture.
    async fn by_id_serving(server: &MockServer, body: String) {
        Mock::given(method("GET"))
            .and(path("/api/v1/users/7/info/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }

    fn web_profile(server: &MockServer) -> String {
        format!(
            r#"{{"data":{{"user":{{"id":"7","username":"someone",
               "profile_pic_url":"{uri}/small.jpg",
               "profile_pic_url_hd":"{uri}/hd.jpg"}}}}}}"#,
            uri = server.uri()
        )
    }

    /// The whole reason for the extra request: `profile_pic_url_hd` is 320x320
    /// despite the name, and only the by-id endpoint offers 1080x1080.
    #[tokio::test]
    async fn it_takes_the_full_size_picture_over_the_one_called_hd() {
        let server = MockServer::start().await;
        profile_serving(&server, web_profile(&server)).await;
        by_id_serving(
            &server,
            format!(
                r#"{{"user":{{"username":"someone","hd_profile_pic_url_info":
                   {{"url":"{uri}/full.jpg","width":1080,"height":1080}}}}}}"#,
                uri = server.uri()
            ),
        )
        .await;

        let (picture, reserved) = fetch_with(&server, "@someone").await;
        let picture = picture.unwrap();

        assert_eq!(picture.bytes, JPEG);
        assert_eq!(picture.username, "someone");
        assert!(picture.url.ends_with("/full.jpg"), "{}", picture.url);
        // The profile, the by-id lookup, and the picture.
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
        // Two of those go to Instagram's API, and each takes its own slot. The
        // CDN is somebody else's server and does not count.
        assert_eq!(reserved, 2);
    }

    /// The full-size lookup is an improvement, not a requirement: if it fails
    /// or says nothing, the smaller picture is still worth having.
    #[tokio::test]
    async fn without_the_full_size_it_falls_back_to_the_smaller_one() {
        let server = MockServer::start().await;
        profile_serving(&server, web_profile(&server)).await;
        by_id_serving(&server, r#"{"user":{"username":"someone"}}"#.into()).await;

        let picture = fetch_with(&server, "someone").await.0.unwrap();
        assert!(picture.url.ends_with("/hd.jpg"), "{}", picture.url);
        assert!(
            matches!(picture.source, Source::Smaller { .. }),
            "falling back has to be recorded, or the command says nothing about having served the one size it exists to avoid"
        );
    }

    /// A by-id endpoint that errors outright must not lose the picture either.
    #[tokio::test]
    async fn a_failing_by_id_lookup_does_not_sink_the_download() {
        let server = MockServer::start().await;
        profile_serving(&server, web_profile(&server)).await;
        Mock::given(method("GET"))
            .and(path("/api/v1/users/7/info/"))
            .respond_with(ResponseTemplate::new(500).set_body_string("nope"))
            .mount(&server)
            .await;

        let picture = fetch_with(&server, "someone").await.0.unwrap();
        assert!(picture.url.ends_with("/hd.jpg"), "{}", picture.url);
        assert!(
            matches!(picture.source, Source::Smaller { .. }),
            "falling back has to be recorded, or the command says nothing about having served the one size it exists to avoid"
        );
    }

    #[tokio::test]
    async fn an_account_with_no_picture_says_so() {
        let server = MockServer::start().await;
        profile_serving(
            &server,
            r#"{"data":{"user":{"id":"7","username":"someone"}}}"#.into(),
        )
        .await;
        by_id_serving(&server, r#"{"user":{"username":"someone"}}"#.into()).await;

        let error = fetch_with(&server, "someone").await.0.unwrap_err();
        assert!(error.to_string().contains("no profile picture"), "{error}");
    }

    /// The session must not reach the CDN. The client test covers the header
    /// itself; this one covers the path `pfp` actually takes.
    #[tokio::test]
    async fn the_picture_request_carries_no_session() {
        let server = MockServer::start().await;
        profile_serving(&server, web_profile(&server)).await;
        by_id_serving(
            &server,
            format!(
                r#"{{"user":{{"username":"someone","hd_profile_pic_url_info":
                   {{"url":"{uri}/full.jpg"}}}}}}"#,
                uri = server.uri()
            ),
        )
        .await;

        fetch_with(&server, "someone").await.0.unwrap();

        let requests = server.received_requests().await.unwrap();
        let to_cdn = requests
            .iter()
            .find(|r| r.url.path() == "/full.jpg")
            .expect("the picture was never requested");
        assert!(to_cdn.headers.get("cookie").is_none());
        // The API requests do carry it; only the CDN is kept clear.
        let to_api = requests
            .iter()
            .find(|r| r.url.path().starts_with("/api/"))
            .unwrap();
        assert!(to_api.headers.get("cookie").is_some());
    }

    #[tokio::test]
    async fn a_name_nobody_owns_is_named_in_the_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"data":{"user":null}}"#))
            .mount(&server)
            .await;

        let error = fetch_with(&server, "someone").await.0.unwrap_err();
        assert!(error.to_string().contains("someone"), "{error}");
    }

    /// Instagram's signed URLs say `.webp` while instructing the CDN to return
    /// JPEG, so the bytes are the only honest source for the name.
    #[test]
    fn the_file_is_named_after_the_bytes_not_the_url() {
        let picture = Picture {
            // Deliberately disagreeing with the bytes: the URL must not decide.
            url: "https://cdn.example/x/abc.webp?stp=dst-jpg".into(),
            ..for_tests("someone", JPEG.to_vec())
        };
        assert_eq!(picture.extension(), "jpg");
    }
}
