//! How a test drives the real app from outside: a channel only this user
//! reaches, in the app's own data directory, that reads the window as a
//! screen reader would, acts on a control by its name, speaks for the system
//! and takes a picture. **Only a `testing` build has it**, so a release
//! binary opens no such socket whatever it is started with.
//!
//! One JSON object a line each way: a [`Request`], then its [`Answer`].

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::base::test_support;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, App, AsyncApp, ElementId, Entity, px, size};
use serde::{Deserialize, Serialize};
use snob_engine::local_socket::{self, Endpoint, Listener};
use snob_store::paths::AppPaths;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::system::Event;
use crate::system::simulated::Simulated;
use crate::ui::shell::Shell;

/// Where the app listens for a test.
pub const CONTROL: Endpoint = Endpoint {
    file: "desktop-control",
    pipe: "desktop-control",
};

/// What a test asks of the app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Request {
    /// Reads the window: the screen, the status, every control.
    Look,
    /// Presses the control `id` with the pointer, in its middle.
    Click { id: String },
    /// Presses a key, with its modifiers: `tab`, `shift-tab`, `ctrl-,`.
    Press { key: String },
    /// Types `text` where focus is.
    Type { text: String },
    /// Puts `text` on the clipboard, as the person copying it elsewhere.
    Copy { text: String },
    /// Has the system say something.
    System { say: Said },
    /// Draws the window to a PNG file.
    Capture { to: PathBuf },
    /// Makes the window this large, in logical pixels.
    Resize { width: f32, height: f32 },
    /// Closes the app.
    Quit,
}

/// What a test has the system say.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Said {
    Online(bool),
    Sleeping,
    /// Back, after this many seconds.
    Woke(u64),
    ShuttingDown,
    /// Another start of the app handed over.
    Activate,
    Dark(bool),
    MoreContrast(bool),
    ReduceMotion(bool),
    ReduceTransparency(bool),
    TextScale(f32),
    Locale(String),
}

/// A control, as a screen reader is told it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// What it is found by: its own id, `name#3` for one of a list.
    pub id: String,
    /// Every id above it, outermost first.
    pub inside: Vec<String>,
    pub role: Option<String>,
    pub label: Option<String>,
    pub value: Option<String>,
    pub description: Option<String>,
    /// Left, top, width and height, in logical pixels of the window.
    pub bounds: [f32; 4],
    pub visible: bool,
    pub focused: Option<bool>,
    pub disabled: Option<bool>,
    pub expanded: Option<bool>,
    pub checked: Option<bool>,
}

/// What the app answers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// Why the request was not done, when it was not.
    pub error: Option<String>,
    /// The screen shown, as the log names it.
    pub screen: String,
    /// What the band says, in the app's words.
    pub status: Option<String>,
    /// What a screen reader was last told.
    pub announced: String,
    /// The window's width and height, in logical pixels.
    pub window: [f32; 2],
    pub nodes: Vec<Node>,
    /// What is on the clipboard.
    pub clipboard: Option<String>,
    /// The notifications shown so far, as `title: body`.
    pub notified: Vec<String>,
    /// The folders opened so far.
    pub opened: Vec<PathBuf>,
    /// How many times the app asked to be started again.
    pub restarts: usize,
}

/// A request on its way to the window's thread, and where its answer goes.
pub type Asked = (Request, oneshot::Sender<Answer>);

/// How long a connection may take to say a whole line.
const PATIENCE: Duration = Duration::from_secs(30);

/// Listens for tests until dropped, handing each request to the window's
/// thread through `asked`.
pub async fn serve(mut listener: Listener, asked: mpsc::UnboundedSender<Asked>) {
    loop {
        let stream = match listener.accept().await {
            Ok(stream) => stream,
            Err(e) => {
                tracing::warn!(error = %e, "the app stopped listening for a test");
                return;
            }
        };
        let asked = asked.clone();
        tokio::spawn(async move {
            let (reading, mut writing) = tokio::io::split(stream);
            let mut lines = BufReader::new(reading).lines();
            while let Ok(Ok(Some(line))) = tokio::time::timeout(PATIENCE, lines.next_line()).await {
                let answer = match serde_json::from_str::<Request>(&line) {
                    Ok(request) => {
                        let (answer, answered) = oneshot::channel();
                        if asked.send((request, answer)).is_err() {
                            return;
                        }
                        answered.await.unwrap_or_else(|_| Answer {
                            error: Some("the app went away".to_string()),
                            ..Answer::default()
                        })
                    }
                    Err(e) => Answer {
                        error: Some(format!("not a request: {e}")),
                        ..Answer::default()
                    },
                };
                let mut line = serde_json::to_vec(&answer).unwrap_or_default();
                line.push(b'\n');
                if writing.write_all(&line).await.is_err() || writing.flush().await.is_err() {
                    return;
                }
            }
        });
    }
}

/// Claims the control channel of the app in `paths`: `None` when another
/// app already listens there.
pub fn bind(paths: &AppPaths) -> std::io::Result<Option<Listener>> {
    local_socket::bind(paths, &CONTROL)
}

/// An id as a test writes it: `name`, or `name#3` for one of a list.
fn element_id(id: &str) -> ElementId {
    match id.rsplit_once('#') {
        Some((name, index)) => match index.parse::<u64>() {
            Ok(index) => ElementId::NamedInteger(name.to_string().into(), index),
            Err(_) => ElementId::Name(id.to_string().into()),
        },
        None => ElementId::Name(id.to_string().into()),
    }
}

fn id_of(id: &ElementId) -> Option<String> {
    match id {
        ElementId::Name(name) => Some(name.to_string()),
        ElementId::NamedInteger(name, index) => Some(format!("{name}#{index}")),
        _ => None,
    }
}

/// What the window's thread needs to answer a test.
pub struct Driven {
    pub window: AnyWindowHandle,
    pub shell: Entity<Shell>,
    /// The system the test speaks for, when the app was started on one.
    pub system: Option<Simulated>,
}

impl Driven {
    fn look(&self, cx: &mut App) -> Answer {
        let shell = self.shell.read(cx);
        let mut answer = Answer {
            screen: shell.screen_name().to_string(),
            status: shell.status().map(|status| {
                crate::ui::frame::status_line(status, crate::ui::look::Look::now(cx)).1
            }),
            announced: shell.announced.to_string(),
            ..Answer::default()
        };
        if let Some(system) = &self.system {
            answer.clipboard = system.clipboard();
            answer.notified = system
                .notified()
                .into_iter()
                .map(|shown| format!("{}: {}", shown.title, shown.body))
                .collect();
            answer.opened = system.opened();
            answer.restarts = system.restarts();
        }
        let read = self.window.update(cx, |_, window, cx| {
            window.render_frame(cx);
            let viewport = window.viewport_size();
            let nodes = test_support::snapshots(window)
                .iter()
                .filter_map(|found| {
                    let named: Vec<String> = found.path().iter().filter_map(id_of).collect();
                    let (id, inside) = named.split_last()?;
                    let bounds = found.bounds();
                    Some(Node {
                        id: id.clone(),
                        inside: inside.to_vec(),
                        role: found.role().map(|role| format!("{role:?}")),
                        label: found.label().map(str::to_string),
                        value: found.value().map(str::to_string),
                        description: found.description().map(str::to_string),
                        bounds: [
                            bounds.origin.x.into(),
                            bounds.origin.y.into(),
                            bounds.size.width.into(),
                            bounds.size.height.into(),
                        ],
                        visible: found.visible(),
                        focused: found.focused(),
                        disabled: found.disabled(),
                        expanded: found.expanded(),
                        checked: found.checked(),
                    })
                })
                .collect();
            ([viewport.width.into(), viewport.height.into()], nodes)
        });
        match read {
            Ok((window, nodes)) => {
                answer.window = window;
                answer.nodes = nodes;
            }
            Err(e) => answer.error = Some(format!("the window is gone: {e}")),
        }
        answer
    }

    /// Does what `request` asks, and answers with the window as it is after.
    fn act(&self, request: Request, cx: &mut App) -> Answer {
        tracing::debug!(?request, "a test asked");
        let done: Result<(), String> = match request {
            Request::Look => Ok(()),
            Request::Click { id } => self
                .window
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    let target = element_id(&id);
                    match window.try_find(target.clone()) {
                        Some(found) if found.visible() => {
                            window.click(target, cx);
                            Ok(())
                        }
                        Some(_) => Err(format!("{id} is not visible")),
                        None => Err(format!("nothing is called {id}")),
                    }
                })
                .unwrap_or_else(|e| Err(e.to_string())),
            Request::Press { key } => self
                .window
                .update(cx, |_, window, cx| window.press(&key, cx))
                .map_err(|e| e.to_string()),
            Request::Type { text } => self
                .window
                .update(cx, |_, window, cx| window.input(&text, cx))
                .map_err(|e| e.to_string()),
            Request::Copy { text } => match &self.system {
                Some(system) => {
                    system.put_on_clipboard(&text);
                    Ok(())
                }
                None => Err("the clipboard is the real one: not a test's".to_string()),
            },
            Request::System { say } => match &self.system {
                Some(system) => {
                    match say {
                        Said::Online(online) => system.say(Event::Online(online)),
                        Said::Sleeping => system.say(Event::Sleeping),
                        Said::Woke(secs) => system.say(Event::Woke {
                            slept: Duration::from_secs(secs),
                        }),
                        Said::ShuttingDown => system.say(Event::ShuttingDown),
                        Said::Activate => system.say(Event::Activate),
                        Said::Dark(dark) => system.change(|settings| settings.dark = dark),
                        Said::MoreContrast(more) => {
                            system.change(|settings| settings.more_contrast = more);
                        }
                        Said::ReduceMotion(reduce) => {
                            system.change(|settings| settings.reduce_motion = reduce);
                        }
                        Said::ReduceTransparency(reduce) => {
                            system.change(|settings| settings.reduce_transparency = reduce);
                        }
                        Said::TextScale(scale) => {
                            system.change(|settings| settings.text_scale = scale);
                        }
                        Said::Locale(locale) => system.change(|settings| settings.locale = locale),
                    }
                    Ok(())
                }
                None => Err("the system is the real one: nothing speaks for it".to_string()),
            },
            Request::Capture { to } => self
                .window
                .update(cx, |_, window, cx| {
                    // **The pointer is taken off the window first**: the
                    // real one lies wherever the person left it, and a
                    // control under it would be pictured hovered.
                    window.dispatch_event(
                        gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                            position: gpui_kit::point(px(-100.), px(-100.)),
                            pressed_button: None,
                            modifiers: gpui_kit::Modifiers::default(),
                        }),
                        cx,
                    );
                    window.render_frame(cx);
                    window
                        .render_to_image()
                        .and_then(|picture| Ok(picture.save(&to)?))
                        .map_err(|e| format!("no picture: {e}"))
                })
                .unwrap_or_else(|e| Err(e.to_string())),
            Request::Resize { width, height } => self
                .window
                .update(cx, |_, window, _| {
                    window.resize(size(px(width), px(height)))
                })
                .map_err(|e| e.to_string()),
            Request::Quit => {
                cx.quit();
                Ok(())
            }
        };
        let mut answer = self.look(cx);
        if let Err(why) = done {
            answer.error = Some(why);
        }
        answer
    }

    /// Answers tests from the window's thread until nobody asks any more.
    pub async fn answer(self, mut asked: mpsc::UnboundedReceiver<Asked>, cx: &mut AsyncApp) {
        while let Some((request, answer)) = asked.recv().await {
            let answered = cx.update(|cx| self.act(request, cx));
            let _ = answer.send(answered);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_one_line_of_json_a_test_can_write_by_hand() {
        for (line, request) in [
            (r#"{"do":"look"}"#, Request::Look),
            (
                r#"{"do":"click","id":"start-paste"}"#,
                Request::Click {
                    id: "start-paste".into(),
                },
            ),
            (
                r#"{"do":"system","say":{"online":false}}"#,
                Request::System {
                    say: Said::Online(false),
                },
            ),
            (
                r#"{"do":"system","say":"sleeping"}"#,
                Request::System {
                    say: Said::Sleeping,
                },
            ),
            (
                r#"{"do":"system","say":{"woke":7200}}"#,
                Request::System {
                    say: Said::Woke(7200),
                },
            ),
        ] {
            assert_eq!(serde_json::from_str::<Request>(line).unwrap(), request);
            assert_eq!(serde_json::to_string(&request).unwrap(), line);
        }
    }

    #[test]
    fn a_control_of_a_list_is_found_by_its_name_and_its_place() {
        assert_eq!(
            element_id("menu-account#2"),
            ElementId::NamedInteger("menu-account".into(), 2)
        );
        assert_eq!(
            element_id("start-paste"),
            ElementId::Name("start-paste".into())
        );
        assert_eq!(
            id_of(&element_id("menu-account#2")).as_deref(),
            Some("menu-account#2")
        );
    }
}
