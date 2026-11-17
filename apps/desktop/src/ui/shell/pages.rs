//! The pages a section opens: an account's profile, its lists, the picture
//! large over them, and what each does. **A list of another account is
//! asked about first**, in the app's own question, and only a yes walks it
//! (`state::profile::asks_first`). **Nothing is read twice on the way
//! back**: a page keeps what it read for as long as the way back holds it.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, Bounds, Context, Entity, FocusHandle, Pixels, Point, ScrollHandle,
    SharedString, UniformListScrollHandle, WeakEntity, Window,
};
use snob_core::Pk;
use snob_ig::pace::CancelToken;
use tokio::sync::watch;

use super::Shell;
use crate::backend::{Listed, Profile, Tiles, Unread, Walked};
use crate::state::flow::{Event, Named, Screen};
use crate::state::profile::{self, Layout, PictureClick, Showing, Side, Tab, Zoom};
use crate::ui::look::Look;

/// How a tab's first page stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TabRead {
    Reading,
    Read(Tiles),
    Failed(Unread),
}

/// A profile while the way back holds it: what was kept of it, then what
/// was read, and each tab shown.
pub(crate) struct ProfilePage {
    pub who: Named,
    pub profile: Option<Profile>,
    /// Since when the read came: what it changed comes in.
    pub read_at: Option<Instant>,
    pub failed: Option<Unread>,
    pub tab: Tab,
    /// The tab the underline leaves, and since when.
    pub tab_from: (Tab, Instant),
    pub tabs: HashMap<Tab, TabRead>,
    /// How the posts were last shown, and since when: they come in again
    /// when that changes.
    pub shown_since: Instant,
    /// The posts' own scroll in the split layout.
    pub scroll: ScrollHandle,
    pub highlights: ScrollHandle,
    /// What opens the picture and what opens the options: where focus goes
    /// back when what they opened closes.
    pub picture_button: FocusHandle,
    pub options_button: FocusHandle,
    /// Where each was last drawn: what its menu opens under.
    pub picture_drawn: Rc<Cell<Bounds<Pixels>>>,
    pub options_drawn: Rc<Cell<Bounds<Pixels>>>,
}

/// One of an account's lists, as far as it is read. **Read on until nothing
/// shows it any more**: the page it is on left and its window closed, it
/// stops, and what it read is kept for the next time.
pub struct PeopleList {
    pub owner: Named,
    pub side: Side,
    pub walked: Walked,
    pub ended: Option<Result<Listed, Unread>>,
    /// When each run of rows came: the first `n` by then.
    pub arrived: Vec<(usize, Instant)>,
    /// Its own window, once it was moved out to one.
    pub window: Option<AnyWindowHandle>,
    pub scroll: UniformListScrollHandle,
    stop: CancelToken,
}

impl PeopleList {
    /// When the row at `at` came.
    pub fn arrived_at(&self, at: usize) -> Option<Instant> {
        self.arrived
            .iter()
            .find(|(until, _)| at < *until)
            .map(|(_, when)| *when)
    }

    fn take(&mut self, walked: Walked) {
        let before = self.walked.people.len();
        if walked.people.len() > before {
            self.arrived.push((walked.people.len(), Instant::now()));
        }
        self.walked = walked;
    }
}

impl Drop for PeopleList {
    fn drop(&mut self) {
        // What it read is saved page by page: stopping keeps it.
        self.stop.cancel();
    }
}

/// The picture of an account, large over the screen.
pub(crate) struct PictureViewer {
    pub who: Named,
    /// The small picture, shown until the large one is on disk.
    pub small: Option<PathBuf>,
    pub large: Option<PathBuf>,
    pub zoom: Zoom,
    pub since: Instant,
    pub focus: FocusHandle,
    /// Where a drag that moves the picture started, while one does.
    pub dragging: Option<Point<Pixels>>,
    /// Where the menu of a right click is, while it is open.
    pub menu: Option<Point<Pixels>>,
    pub menu_focus: FocusHandle,
    /// What was last done, said under the buttons.
    pub said: Option<SharedString>,
    /// What had focus when it opened, where it goes back.
    pub back_to: Option<FocusHandle>,
}

/// Which small menu is open over a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopupKind {
    /// A story is up: the story or the picture.
    Picture,
    /// What can be done with the account, or known of it.
    Options,
}

/// A small menu over a profile, while it is open.
pub(crate) struct Popup {
    pub kind: PopupKind,
    pub focus: FocusHandle,
    pub since: Instant,
}

/// The question reading another account's list asks first.
pub(crate) struct ListQuestion {
    pub owner: Named,
    pub side: Side,
    pub focus: FocusHandle,
    /// The answer focus starts on: the one that reads nothing.
    pub cancel: FocusHandle,
    pub from: Option<FocusHandle>,
}

/// Everything the pages hold, apart from the rest of the shell.
#[derive(Default)]
pub(crate) struct Pages {
    pub profiles: HashMap<Pk, ProfilePage>,
    pub lists: HashMap<(Pk, Side), Entity<PeopleList>>,
    pub viewer: Option<PictureViewer>,
    pub popup: Option<Popup>,
    pub asking: Option<ListQuestion>,
    /// How profiles are laid out and their posts shown, the same for every
    /// one this run.
    pub layout: Layout,
    pub showing: Showing,
    /// Since when the layout is what it is: the page comes in again.
    pub laid_out_at: Option<Instant>,
    /// The lists moved out to windows of their own: **one walk per list**,
    /// so opening one of them again shows the one going.
    pub detached: Vec<WeakEntity<PeopleList>>,
}

impl Shell {
    /// To the profile of the account in use.
    pub(crate) fn open_own_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.active().map(|account| account.named.clone()) else {
            return;
        };
        self.close_the_menu(window, cx);
        self.open_profile(account, window, cx);
    }

    /// To `who`'s profile.
    pub(crate) fn open_profile(&mut self, who: Named, window: &mut Window, cx: &mut Context<Self>) {
        if !self.flow.screen().signed_in() {
            return;
        }
        tracing::info!(action = "open_profile", "action");
        self.pages.popup = None;
        self.happened(Event::OpenProfile(who), window, cx);
    }

    /// Whether there is a step to go back to: what the band's arrow says.
    pub(crate) fn can_go_back(&self) -> bool {
        self.has_a_menu()
            && (self.is_editing() || self.has_an_automation_open() || self.flow.can_go_back())
    }

    /// One step back: out of the editor (which asks first about what is
    /// not finished), out of an automation's page, or to the screen before.
    pub(crate) fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_a_menu() {
            return;
        }
        if self.is_editing() {
            self.leave_the_editor(window, cx);
        } else if self.has_an_automation_open() {
            self.close_the_automation(window, cx);
        } else if self.flow.can_go_back() {
            tracing::info!(action = "back", "action");
            self.menu = None;
            self.happened(Event::Back, window, cx);
        }
    }

    /// The page of the screen just shown, made when it is new, and every
    /// page the way back no longer holds let go: **a list let go stops**.
    pub(super) fn settle_the_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let held: Vec<Screen> = self
            .flow
            .trail()
            .iter()
            .chain(std::iter::once(self.flow.screen()))
            .cloned()
            .collect();
        self.pages.profiles.retain(|pk, _| {
            held.iter()
                .any(|screen| matches!(screen, Screen::Profile(who) if who.pk == *pk))
        });
        self.pages.lists.retain(|(pk, side), _| {
            held.iter().any(|screen| {
                matches!(screen, Screen::People { owner, side: shown } if owner.pk == *pk && shown == side)
            })
        });
        self.pages.popup = None;
        self.pages.viewer = None;
        match self.shown.clone() {
            Screen::Profile(who) if !self.pages.profiles.contains_key(&who.pk) => {
                self.read_the_profile(who, window, cx);
            }
            Screen::People { owner, side } if !self.pages.lists.contains_key(&(owner.pk, side)) => {
                self.pages.detached.retain(|list| list.upgrade().is_some());
                let going = self.pages.detached.iter().find_map(|list| {
                    let list = list.upgrade()?;
                    let same = {
                        let read = list.read(cx);
                        read.owner.pk == owner.pk && read.side == side
                    };
                    same.then_some(list)
                });
                match going {
                    Some(list) => {
                        self.pages.lists.insert((owner.pk, side), list);
                    }
                    None => self.walk_the_list(owner, side, window, cx),
                }
            }
            _ => {}
        }
    }

    /// Shows what was kept of `who` at once, and reads it now.
    fn read_the_profile(&mut self, who: Named, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.pages.profiles.insert(
            who.pk,
            ProfilePage {
                who: who.clone(),
                profile: None,
                read_at: None,
                failed: None,
                tab: Tab::Posts,
                tab_from: (Tab::Posts, now),
                tabs: HashMap::new(),
                shown_since: now,
                scroll: ScrollHandle::new(),
                highlights: ScrollHandle::new(),
                picture_button: cx.focus_handle(),
                options_button: cx.focus_handle(),
                picture_drawn: Rc::default(),
                options_drawn: Rc::default(),
            },
        );
        let pk = who.pk;
        let kept = self.backend.profile_kept(who.clone());
        let reading = self.backend.profile(who);
        self.engine_asked(cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Some(kept)) = kept.await {
                this.update(cx, |this, cx| {
                    if let Some(page) = this.pages.profiles.get_mut(&pk)
                        && page.profile.as_ref().is_none_or(|shown| !shown.read)
                    {
                        page.profile = Some(kept);
                        cx.notify();
                    }
                })
                .ok();
            }
            let read = reading.await;
            this.update_in(cx, |this, window, cx| {
                this.engine_answered(cx);
                this.profile_read(pk, read.ok(), window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn profile_read(
        &mut self,
        pk: Pk,
        read: Option<Result<Profile, Unread>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.pages.profiles.get_mut(&pk) else {
            return;
        };
        match read {
            Some(Ok(profile)) => {
                // An account's own page read now is what its menu shows too.
                if let Some(account) = self.accounts.iter_mut().find(|a| a.named.pk == pk) {
                    account.full_name = profile.full_name.clone().or(account.full_name.take());
                    account.biography = profile.biography.clone().or(account.biography.take());
                    if account.picture.is_none() {
                        account.picture = profile.picture.clone();
                    }
                }
                let visible = profile.visible == Some(true);
                page.profile = Some(profile);
                page.read_at = Some(Instant::now());
                page.failed = None;
                if visible {
                    let tab = page.tab;
                    self.read_the_tab(pk, tab, window, cx);
                }
            }
            Some(Err(why)) => {
                let said = super::super::screens::profile::unread_words(&why, Look::now(cx));
                page.failed = Some(why);
                self.announce(said, cx);
            }
            None => page.failed = Some(Unread::Failed(String::new())),
        }
        cx.notify();
    }

    /// Reads the first page of `tab` of the profile `pk`, once.
    fn read_the_tab(&mut self, pk: Pk, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.pages.profiles.get_mut(&pk) else {
            return;
        };
        if page.tabs.contains_key(&tab) {
            return;
        }
        let who = page
            .profile
            .as_ref()
            .map_or_else(|| page.who.clone(), |profile| profile.named.clone());
        page.tabs.insert(tab, TabRead::Reading);
        let reading = self.backend.tiles(who, tab);
        self.engine_asked(cx);
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            this.update(cx, |this, cx| {
                this.engine_answered(cx);
                if let Some(page) = this.pages.profiles.get_mut(&pk) {
                    let read = match read {
                        Ok(Ok(tiles)) => TabRead::Read(tiles),
                        Ok(Err(why)) => TabRead::Failed(why),
                        Err(_) => TabRead::Failed(Unread::Failed(String::new())),
                    };
                    page.tabs.insert(tab, read);
                    page.shown_since = Instant::now();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The profile page shown, when one is.
    pub(crate) fn profile_page(&self) -> Option<&ProfilePage> {
        match &self.shown {
            Screen::Profile(who) => self.pages.profiles.get(&who.pk),
            _ => None,
        }
    }

    /// To tab `tab` of the profile shown.
    pub(crate) fn choose_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Profile(who) = self.shown.clone() else {
            return;
        };
        let Some(page) = self.pages.profiles.get_mut(&who.pk) else {
            return;
        };
        if page.tab == tab {
            return;
        }
        tracing::info!(action = "choose_profile_tab", "action");
        page.tab_from = (page.tab, Instant::now());
        page.tab = tab;
        page.shown_since = Instant::now();
        let visible = page
            .profile
            .as_ref()
            .is_some_and(|profile| profile.visible == Some(true));
        if visible {
            self.read_the_tab(who.pk, tab, window, cx);
        }
        cx.notify();
    }

    /// Shows the posts as a grid or as a list.
    pub(crate) fn show_posts_as(&mut self, showing: Showing, cx: &mut Context<Self>) {
        if self.pages.showing == showing {
            return;
        }
        tracing::info!(action = "show_posts_as", ?showing, "action");
        self.pages.showing = showing;
        if let Screen::Profile(who) = &self.shown
            && let Some(page) = self.pages.profiles.get_mut(&who.pk)
        {
            page.shown_since = Instant::now();
        }
        cx.notify();
    }

    /// The other layout.
    pub(crate) fn change_the_layout(&mut self, cx: &mut Context<Self>) {
        self.pages.layout = self.pages.layout.other();
        tracing::info!(action = "change_profile_layout", layout = ?self.pages.layout, "action");
        self.pages.laid_out_at = Some(Instant::now());
        cx.notify();
    }

    /// The profile shown scrolls its posts on its own: in the split layout.
    pub(super) fn a_page_scrolls_itself(&self) -> bool {
        matches!(self.shown, Screen::People { .. })
            || (matches!(self.shown, Screen::Profile(_)) && self.pages.layout == Layout::Split)
    }

    /// A click on the picture of the profile shown: the picture large, or,
    /// with a story up, the choice of which.
    pub(crate) fn click_the_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.profile_page().and_then(|page| page.profile.clone()) else {
            return;
        };
        match profile::picture_click(profile.story_up) {
            PictureClick::Open => self.open_the_picture(window, cx),
            PictureClick::Choose => self.open_a_popup(PopupKind::Picture, window, cx),
        }
    }

    /// Opens a small menu over the profile, or closes the one open.
    pub(crate) fn open_a_popup(
        &mut self,
        kind: PopupKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .pages
            .popup
            .as_ref()
            .is_some_and(|popup| popup.kind == kind)
        {
            self.close_the_popup(window, cx);
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.pages.popup = Some(Popup {
            kind,
            focus,
            since: Instant::now(),
        });
        cx.notify();
    }

    /// Closes the small menu, focus back on what opened it.
    pub(crate) fn close_the_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = self.pages.popup.take() else {
            return;
        };
        if let Some(page) = self.profile_page() {
            let back = match popup.kind {
                PopupKind::Picture => page.picture_button.clone(),
                PopupKind::Options => page.options_button.clone(),
            };
            window.focus(&back, cx);
        }
        cx.notify();
    }

    /// The story's way in: **not opened yet, and nothing of it is read or
    /// marked as seen**. Says so.
    pub(crate) fn open_the_story(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!(action = "open_story", "action");
        self.close_the_popup(window, cx);
        let said = Look::now(cx).tr("profile-story-later");
        self.announce(said, cx);
    }

    /// Copies something the options offer: the username, the address of the
    /// profile, the account's number.
    pub(crate) fn copy_of_the_profile(
        &mut self,
        what: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.profile_page().and_then(|page| page.profile.clone()) else {
            return;
        };
        let username = profile.named.username.clone().unwrap_or_default();
        let (text, said) = match what {
            "username" => (username, "profile-copied-username"),
            "link" => (
                format!("https://www.instagram.com/{username}/"),
                "profile-copied-link",
            ),
            _ => (profile.named.pk.to_string(), "profile-copied-id"),
        };
        tracing::info!(action = "copy_from_profile", what, "action");
        self.system.copy(&text, cx);
        self.close_the_popup(window, cx);
        let said = Look::now(cx).tr(said);
        self.announce(said, cx);
    }

    /// The picture of the profile shown, large over the screen.
    pub(crate) fn open_the_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.profile_page() else {
            return;
        };
        let Some(profile) = page.profile.clone() else {
            return;
        };
        tracing::info!(action = "open_profile_picture", "action");
        let back_to = Some(page.picture_button.clone());
        self.pages.popup = None;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let who = profile.named.clone();
        self.pages.viewer = Some(PictureViewer {
            who: who.clone(),
            small: profile.picture.clone(),
            large: None,
            zoom: Zoom::FIT,
            since: Instant::now(),
            focus,
            dragging: None,
            menu: None,
            menu_focus: cx.focus_handle(),
            said: None,
            back_to,
        });
        let fetching = self.backend.full_picture(who.clone());
        self.engine_asked(cx);
        cx.spawn_in(window, async move |this, cx| {
            let large = fetching.await;
            this.update(cx, |this, cx| {
                this.engine_answered(cx);
                let Ok(large) = large else {
                    return;
                };
                if let Some(viewer) = &mut this.pages.viewer
                    && viewer.who.pk == who.pk
                {
                    viewer.large = large;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Closes the picture, focus back where it was.
    pub(crate) fn close_the_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(viewer) = self.pages.viewer.take() {
            if let Some(back) = viewer.back_to {
                window.focus(&back, cx);
            }
            cx.notify();
        }
    }

    /// The wheel over the picture: zoomed about the pointer.
    pub(crate) fn zoom_the_picture(
        &mut self,
        notches: f32,
        at: (f32, f32),
        size: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        if let Some(viewer) = &mut self.pages.viewer {
            viewer.zoom = viewer.zoom.wheeled(notches, at, size);
            cx.notify();
        }
    }

    /// A drag over the picture, zoomed in: it follows.
    pub(crate) fn drag_the_picture(
        &mut self,
        to: Point<Pixels>,
        size: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        if let Some(viewer) = &mut self.pages.viewer
            && let Some(from) = viewer.dragging
        {
            let by = (f32::from(to.x - from.x), f32::from(to.y - from.y));
            viewer.zoom = viewer.zoom.dragged(by, size);
            viewer.dragging = Some(to);
            cx.notify();
        }
    }

    /// A drag over the picture starts at `at`, or, with none, ends.
    pub(crate) fn hold_the_picture(&mut self, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        if let Some(viewer) = &mut self.pages.viewer {
            viewer.dragging = at.filter(|_| viewer.zoom.zoomed());
            cx.notify();
        }
    }

    /// The picture back as it opened.
    pub(crate) fn fit_the_picture(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = &mut self.pages.viewer {
            viewer.zoom = Zoom::FIT;
            cx.notify();
        }
    }

    /// Opens the menu a right click on the picture opens, at `at`.
    pub(crate) fn picture_menu(
        &mut self,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(viewer) = &mut self.pages.viewer {
            viewer.menu = at;
            let focus = if at.is_some() {
                viewer.menu_focus.clone()
            } else {
                viewer.focus.clone()
            };
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    /// Saves the picture shown in the system's downloads, or with `ask` in
    /// a folder the person chooses, and says where.
    pub(crate) fn save_the_picture(
        &mut self,
        ask: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(viewer) = &mut self.pages.viewer else {
            return;
        };
        viewer.menu = None;
        let who = viewer.who.clone();
        window.focus(&viewer.focus.clone(), cx);
        tracing::info!(action = "save_profile_picture", ask, "action");
        let folder = if ask {
            self.system.choose_folder(cx)
        } else {
            crate::backend::answered(self.system.downloads())
        };
        let backend = self.backend.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Some(folder)) = folder.await else {
                return;
            };
            let Ok(saved) = backend.save_picture(who, folder).await else {
                return;
            };
            this.update(cx, |this, cx| {
                let look = Look::now(cx).clone();
                let said = match &saved {
                    Some(path) => look.tr_with(
                        "viewer-saved",
                        &[(
                            "file",
                            &path.file_name().unwrap_or_default().to_string_lossy(),
                        )],
                    ),
                    None => look.tr("viewer-not-saved"),
                };
                if let Some(viewer) = &mut this.pages.viewer {
                    viewer.said = Some(said.clone().into());
                }
                this.announce(said, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Puts the picture shown on the clipboard, and says so.
    pub(crate) fn copy_the_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(viewer) = &mut self.pages.viewer else {
            return;
        };
        viewer.menu = None;
        window.focus(&viewer.focus.clone(), cx);
        tracing::info!(action = "copy_profile_picture", "action");
        let reading = self.backend.picture_bytes(viewer.who.clone());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(bytes) = reading.await else {
                return;
            };
            this.update(cx, |this, cx| {
                let look = Look::now(cx).clone();
                let said = match bytes {
                    Some(bytes) => {
                        this.system.copy_image(bytes, cx);
                        look.tr("viewer-copied")
                    }
                    None => look.tr("viewer-not-copied"),
                };
                if let Some(viewer) = &mut this.pages.viewer {
                    viewer.said = Some(said.clone().into());
                }
                this.announce(said, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// One of the profile's lists: at once for the account's own, and
    /// **another's only after the question is answered yes**.
    pub(crate) fn open_a_list(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.profile_page().and_then(|page| page.profile.clone()) else {
            return;
        };
        let Some(viewer) = self.active().map(|account| account.named.pk) else {
            return;
        };
        if profile.visible == Some(false) {
            return;
        }
        if profile::asks_first(profile.named.pk, viewer) {
            let from = window.focused(cx);
            let (focus, cancel) = (cx.focus_handle(), cx.focus_handle());
            window.focus(&cancel, cx);
            self.pages.asking = Some(ListQuestion {
                owner: profile.named,
                side,
                focus,
                cancel,
                from,
            });
            cx.notify();
        } else {
            self.happened(
                Event::OpenList {
                    owner: profile.named,
                    side,
                },
                window,
                cx,
            );
        }
    }

    /// The answer was no: nothing is read.
    pub(crate) fn keep_the_list_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(asked) = self.pages.asking.take() {
            tracing::info!(action = "decline_list", "action");
            if let Some(from) = asked.from {
                window.focus(&from, cx);
            }
            cx.notify();
        }
    }

    /// The answer was yes: the list opens and is read.
    pub(crate) fn read_the_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(asked) = self.pages.asking.take() {
            tracing::info!(action = "consent_to_list", "action");
            self.happened(
                Event::OpenList {
                    owner: asked.owner,
                    side: asked.side,
                },
                window,
                cx,
            );
        }
    }

    /// Starts reading a list, its rows shown as they come.
    fn walk_the_list(
        &mut self,
        owner: Named,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stop = CancelToken::default();
        let (said, mut heard) = watch::channel(Walked::default());
        let list = cx.new(|_| PeopleList {
            owner: owner.clone(),
            side,
            walked: Walked::default(),
            ended: None,
            arrived: Vec::new(),
            window: None,
            scroll: UniformListScrollHandle::new(),
            stop: stop.clone(),
        });
        self.pages.lists.insert((owner.pk, side), list.clone());
        let walking = self.backend.list(owner, side, said, stop);
        self.engine_asked(cx);
        let weak = list.downgrade();
        cx.spawn_in(window, async move |this, cx| {
            let mut first = true;
            while heard.changed().await.is_ok() {
                if std::mem::take(&mut first) {
                    this.update(cx, |this, cx| this.engine_answered(cx)).ok();
                }
                let walked = heard.borrow_and_update().clone();
                let told = weak.update(cx, |list, cx| {
                    list.take(walked);
                    cx.notify();
                });
                if told.is_err() {
                    return;
                }
            }
        })
        .detach();
        let weak = list.downgrade();
        cx.spawn_in(window, async move |this, cx| {
            let ended = walking
                .await
                .unwrap_or_else(|_| Err(Unread::Failed(String::new())));
            let said = weak
                .update(cx, |list, cx| {
                    if let Ok(listed) = &ended {
                        let walked = Walked {
                            people: listed.people.clone(),
                            expected: list.walked.expected,
                        };
                        list.take(walked);
                    }
                    list.ended = Some(ended.clone());
                    cx.notify();
                    ended
                })
                .ok();
            if let Some(ended) = said {
                this.update(cx, |this, cx| {
                    this.engine_answered(cx);
                    let look = Look::now(cx).clone();
                    let said = match ended {
                        Ok(listed) => look.tr_with(
                            "people-read",
                            &[("count", &listed.people.len().to_string())],
                        ),
                        Err(why) => super::super::screens::profile::unread_words(&why, &look),
                    };
                    this.announce(said, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// The list shown, when one is.
    pub(crate) fn people_list(&self) -> Option<Entity<PeopleList>> {
        match &self.shown {
            Screen::People { owner, side } => self.pages.lists.get(&(owner.pk, *side)).cloned(),
            _ => None,
        }
    }

    /// Moves the list shown out to a window of its own, and goes back.
    pub(crate) fn detach_the_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(list) = self.people_list() else {
            return;
        };
        tracing::info!(action = "detach_list", "action");
        let main = window.window_handle();
        let shell = cx.entity().downgrade();
        let opened = crate::ui::detached::open(list.clone(), shell, main, cx);
        match opened {
            Ok(handle) => {
                list.update(cx, |list, _| list.window = Some(handle));
                self.pages.detached.push(list.downgrade());
                self.go_back(window, cx);
            }
            Err(e) => tracing::warn!(error = %e, "the list could not have a window of its own"),
        }
    }

    /// Closes whatever of the pages is open over the screen, the newest
    /// first: whether anything was.
    pub(super) fn dismiss_a_page_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .pages
            .viewer
            .as_ref()
            .is_some_and(|viewer| viewer.menu.is_some())
        {
            self.picture_menu(None, window, cx);
        } else if self.pages.viewer.is_some() {
            self.close_the_picture(window, cx);
        } else if self.pages.asking.is_some() {
            self.keep_the_list_closed(window, cx);
        } else if self.pages.popup.is_some() {
            self.close_the_popup(window, cx);
        } else {
            return false;
        }
        true
    }

    /// Whether something of the pages is open over the screen.
    pub(super) fn a_page_overlay_is_open(&self) -> bool {
        self.pages.viewer.is_some() || self.pages.asking.is_some() || self.pages.popup.is_some()
    }
}

/// How far `since` is through `lasts`, eased, asking for another frame
/// while it moves.
pub(crate) fn eased(since: Instant, lasts: std::time::Duration, window: &mut Window) -> f32 {
    let at = since.elapsed().as_secs_f32() / lasts.as_secs_f32().max(1e-3);
    if at < 1.0 {
        window.request_animation_frame();
    }
    crate::theme::motion::ease_out(at)
}
