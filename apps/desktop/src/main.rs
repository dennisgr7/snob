//! The desktop app: one window on the engine the command line runs, linked
//! in process.
//!
//! What `main` does is the order things start in: where the data is, the
//! log, the one instance, the engine's runtime, and then the window. **A
//! `testing` build never uses the real data directory or the real keyring**,
//! whatever it is started with.

// No console window behind the app on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::QuitMode;
use snob_desktop::backend::engine::Engine;
use snob_desktop::instance::{self, Instance};
use snob_desktop::runtime::Runtime;
use snob_desktop::state::prefs::Prefs;
use snob_desktop::system::native::Native;
use snob_desktop::system::{Event, Settings, System};
use snob_desktop::ui::assets::Assets;
use snob_desktop::{front, ui};
use snob_engine::log;
use snob_store::paths::AppPaths;
use snob_store::secrets::SecretStore;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// Where this run keeps its files and its secrets.
struct Wiring {
    paths: AppPaths,
    secrets: SecretStore,
    /// A test build says so in its band.
    testing: bool,
}

#[cfg(not(feature = "testing"))]
fn wiring(_: &[String]) -> anyhow::Result<Wiring> {
    let paths = AppPaths::discover()?;
    Ok(Wiring {
        secrets: SecretStore::new(paths.clone(), false),
        paths,
        testing: false,
    })
}

/// The value after `name` among `args`.
#[cfg(feature = "testing")]
fn value_of<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|arg| arg == name)?;
    args.get(at + 1).map(String::as_str)
}

/// A test build's wiring: everything under `--test-root`, or a directory of
/// this run's own when none is given, and every client pointed at
/// `--instagram` when that is.
#[cfg(feature = "testing")]
fn wiring(args: &[String]) -> anyhow::Result<Wiring> {
    let root = match value_of(args, "--test-root") {
        Some(root) => std::path::PathBuf::from(root),
        None => std::env::temp_dir().join(format!("snob-desktop-test-{}", std::process::id())),
    };
    std::fs::create_dir_all(&root)?;
    let instagram = value_of(args, "--instagram").map(str::parse).transpose()?;
    let sandbox = snob_engine::sandbox::Sandbox::under(&root, instagram, false)?;
    Ok(Wiring {
        secrets: sandbox.secrets(),
        paths: sandbox.paths,
        testing: true,
    })
}

/// The system a test speaks for, when the run was started with
/// `--simulate-system`: set up as the arguments say, and following nothing
/// of the real one.
#[cfg(feature = "testing")]
fn simulated(args: &[String]) -> Option<Settings> {
    let has = |name: &str| args.iter().any(|arg| arg == name);
    has("--simulate-system").then(|| Settings {
        dark: has("--dark"),
        more_contrast: has("--more-contrast"),
        reduce_motion: has("--reduce-motion"),
        reduce_transparency: has("--reduce-transparency"),
        text_scale: value_of(args, "--text-scale")
            .and_then(|scale| scale.parse().ok())
            .unwrap_or(1.0),
        locale: value_of(args, "--locale").unwrap_or("en").to_string(),
    })
}

#[cfg(not(feature = "testing"))]
fn simulated(_: &[String]) -> Option<Settings> {
    None
}

/// The system the app asks, and what brings its window to the front when it
/// is started a second time.
type Wired = (Rc<dyn System>, Box<dyn Fn() + Send>);

/// The system a test speaks for, when the app was started on one.
#[cfg(feature = "testing")]
type Spoken = Option<snob_desktop::system::simulated::Simulated>;
#[cfg(not(feature = "testing"))]
type Spoken = ();

/// The real system, which also follows the theme the window is drawn in.
fn native(cx: &mut gpui_kit::App) -> (Wired, Rc<Native>) {
    let native = Rc::new(Native::new(cx));
    let says = native.says();
    let activate = Box::new(move || says.say(Event::Activate));
    ((native.clone(), activate), native)
}

#[cfg(feature = "testing")]
fn system(
    simulated: Option<Settings>,
    cx: &mut gpui_kit::App,
) -> (Wired, Option<Rc<Native>>, Spoken) {
    use snob_desktop::system::simulated::Simulated;
    match simulated {
        Some(settings) => {
            let system = Simulated::new(settings);
            let says = system.clone();
            let activate = Box::new(move || says.say(Event::Activate));
            ((Rc::new(system.clone()), activate), None, Some(system))
        }
        None => {
            let (wired, native) = native(cx);
            (wired, Some(native), None)
        }
    }
}

#[cfg(not(feature = "testing"))]
fn system(_: Option<Settings>, cx: &mut gpui_kit::App) -> (Wired, Option<Rc<Native>>, Spoken) {
    let (wired, native) = native(cx);
    (wired, Some(native), ())
}

/// Lets a test drive this app: read its window, act on it, speak for its
/// system (`snob_desktop::control`).
#[cfg(feature = "testing")]
fn be_driven(
    paths: &AppPaths,
    runtime: &Runtime,
    window: gpui_kit::AnyWindowHandle,
    shell: gpui_kit::Entity<ui::shell::Shell>,
    system: Spoken,
    cx: &mut gpui_kit::App,
) {
    use snob_desktop::control;
    // A listener is made on the runtime it is listened on.
    let _on_the_runtime = runtime.handle().enter();
    let listener = match control::bind(paths) {
        Ok(Some(listener)) => listener,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(error = %e, "no test can drive this app");
            return;
        }
    };
    let (asked, heard) = tokio::sync::mpsc::unbounded_channel();
    runtime.handle().spawn(control::serve(listener, asked));
    let driven = control::Driven {
        window,
        shell,
        system,
    };
    cx.spawn(async move |cx| driven.answer(heard, cx).await)
        .detach();
}

#[cfg(not(feature = "testing"))]
fn be_driven(
    _: &AppPaths,
    _: &Runtime,
    _: gpui_kit::AnyWindowHandle,
    _: gpui_kit::Entity<ui::shell::Shell>,
    _: Spoken,
    _: &mut gpui_kit::App,
) {
}

/// What opens the window: at the start, and again from the Dock.
type Opens = Box<dyn Fn(&mut gpui_kit::App)>;

/// What this program is, for a crash report and a diagnostic one.
fn about() -> String {
    format!(
        "snob desktop {} on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Wiring {
        paths,
        secrets,
        testing,
    } = wiring(&args)?;
    paths.ensure_dirs()?;

    let started = log::start(&paths, std::env::var("SNOB_LOG").ok().as_deref())?;
    let (detail, files) = (started.detail, started.layer.files());
    tracing_subscriber::registry().with(started.layer).init();
    log::keep_crashes(&log::dir(&paths), about());

    let runtime = Arc::new(Runtime::start()?);
    let listener = match runtime.block_on(instance::claim(&paths))? {
        Instance::First(listener) => listener,
        Instance::Second => {
            tracing::info!("another start of the app is running: handed over to it");
            return Ok(());
        }
    };
    let (_notices, snob_found) = front::hand_over();
    tracing::info!(about = about(), testing, snob_found, "the app started");

    #[cfg(feature = "testing")]
    let listed = value_of(&args, "--browsers").and_then(|count| count.parse().ok());
    let simulated = simulated(&args);
    let reopen: Rc<RefCell<Option<Opens>>> = Rc::default();
    let app = gpui_kit::application().with_assets(Assets);
    {
        let reopen = Rc::clone(&reopen);
        app.on_reopen(move |cx| {
            if cx.windows().is_empty()
                && let Some(open) = &*reopen.borrow()
            {
                tracing::info!("the window is opened again");
                open(cx);
            }
        });
    }
    app.run(move |cx| {
        // On macOS an app whose window was closed stays in the Dock, where a
        // click opens it again; everywhere else closing the window is
        // leaving.
        cx.set_quit_mode(QuitMode::Default);
        let ((system, activate), native, spoken) = system(simulated, cx);
        runtime.handle().spawn(instance::serve(listener, activate));

        ui::window::init(&*system, testing, cx);
        let settings = system.settings();
        tracing::info!(
            dark = settings.dark,
            more_contrast = settings.more_contrast,
            reduce_motion = settings.reduce_motion,
            reduce_transparency = settings.reduce_transparency,
            text_scale = settings.text_scale,
            locale = settings.locale,
            "the system's settings"
        );
        let backend = Engine::new(
            Arc::clone(&runtime),
            paths.clone(),
            secrets,
            (detail, files),
            about(),
        );
        #[cfg(feature = "testing")]
        let backend = match listed {
            Some(count) => backend.listing_browsers(count),
            None => backend,
        };
        let backend = Arc::new(backend);
        let open = {
            let paths = paths.clone();
            move |cx: &mut gpui_kit::App| {
                let placement = Prefs::load(&paths).window;
                let opened = ui::window::open(backend.clone(), system.clone(), placement, cx);
                let (window, shell) = match opened {
                    Ok(opened) => opened,
                    Err(e) => {
                        tracing::error!(error = %e, "the window could not be opened");
                        return None;
                    }
                };
                if let Some(native) = native.clone() {
                    let followed = window.update(cx, |_, window, _| {
                        window
                            .observe_window_appearance(move |window, _| {
                                native.appearance_changed(window.appearance());
                            })
                            .detach();
                    });
                    if let Err(e) = followed {
                        tracing::warn!(error = %e, "the system's theme is not followed");
                    }
                }
                Some((window, shell))
            }
        };
        let Some((window, shell)) = open(cx) else {
            cx.quit();
            return;
        };
        be_driven(&paths, &runtime, window, shell, spoken, cx);
        *reopen.borrow_mut() = Some(Box::new(move |cx| {
            open(cx);
        }));
        cx.on_app_quit(|_| async { tracing::info!("the app stopped") })
            .detach();
    });
    Ok(())
}
