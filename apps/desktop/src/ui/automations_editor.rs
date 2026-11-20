//! What the editor's person does with the automation it holds: puts steps
//! on the canvas, moves and joins them, undoes, configures one in a dialog,
//! and leaves, asked first when what is left cannot run.
//!
//! **Every change is kept at once** (`Shell::keep_the_copy`), and what is
//! asked on leaving is `Editing::leaving`. **A press on the rail or a
//! command that would leave the editor is held until that question is
//! answered** (`Shell::guard_the_editor`).

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, FocusHandle, Pixels, Point, Subscription, Window};
use snob_core::automation::{NodeId, Only, Problem, Step, WebhookFormat};

use super::automations::{Edit, Grab, Item, Opened, Said};
use super::look::Look;
use super::shell::Shell;
use crate::backend::Automation;
use crate::state::editing::{Discard, Leaving, Order, suggested_place};
use crate::state::flow::Event;
use crate::state::schedule::{Advanced, Choices, MOST_COUNT, ScheduleForm};
use crate::state::steps::{ActionForm, WebhookForm};
use crate::ui::parts::Select;
use snob_core::automation::VideoQuality;

/// An arrow being drawn: from which step, to where the pointer is, in the
/// canvas's own units, and the input it sticks to
/// (`state::automations::snapped`), which a release joins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Wire {
    pub from: NodeId,
    pub to: (f32, f32),
    pub snapped: Option<NodeId>,
}

/// The small menu of a step, open where the press was.
pub(crate) struct NodeMenu {
    pub id: NodeId,
    pub at: Point<Pixels>,
    pub focus: FocusHandle,
}

/// The question asked on leaving what cannot run.
pub(crate) struct Leave {
    pub problems: Vec<Problem>,
    /// Where the person was going when the question came: done after a
    /// discard.
    pub then: Option<Event>,
    pub focus: FocusHandle,
    pub cancel: FocusHandle,
}

/// A row of a webhook's headers.
pub(crate) struct HeaderRow {
    pub name: Entity<InputState>,
    pub value: Entity<InputState>,
    _watching: [Subscription; 2],
}

/// A time of day typed in a schedule's dialog.
pub(crate) struct TimeRow {
    pub input: Entity<InputState>,
    _watching: Subscription,
}

/// `watch.toml`'s fields, typed in a schedule's Advanced.
pub(crate) struct AdvancedFields {
    pub every: Entity<InputState>,
    pub on: Entity<InputState>,
    pub at: Entity<InputState>,
    pub months: Entity<InputState>,
    pub days: Entity<InputState>,
    pub cron: Entity<InputState>,
    pub jitter: Entity<InputState>,
}

impl AdvancedFields {
    fn all(&self) -> [&Entity<InputState>; 7] {
        [
            &self.every,
            &self.on,
            &self.at,
            &self.months,
            &self.days,
            &self.cron,
            &self.jitter,
        ]
    }
}

/// A schedule's dialog: what is chosen, and the fields what is typed is
/// read from.
pub(crate) struct ScheduleFields {
    pub form: ScheduleForm,
    pub count: Entity<InputState>,
    pub times: Vec<TimeRow>,
    pub advanced: AdvancedFields,
    /// The month every few months starts from.
    pub first_month: Select,
    _watching: Vec<Subscription>,
}

/// What a dialog asks for, which depends on the step.
pub(crate) enum Fields {
    Schedule(Box<ScheduleFields>),
    Action {
        form: ActionForm,
        username: Entity<InputState>,
        save_to: Entity<InputState>,
    },
    Filter {
        only: Only,
    },
    Webhook {
        url: Entity<InputState>,
        format: WebhookFormat,
        heartbeat: bool,
        headers: Vec<HeaderRow>,
    },
}

/// A step's configuration, open over the window.
pub(crate) struct Dialog {
    pub node: NodeId,
    pub fields: Fields,
    pub focus: FocusHandle,
    pub cancel: FocusHandle,
    _watching: Vec<Subscription>,
}

impl Dialog {
    /// The schedule as chosen, with what is typed in its fields.
    pub fn schedule_form(&self, cx: &gpui_kit::App) -> Option<ScheduleForm> {
        let Fields::Schedule(fields) = &self.fields else {
            return None;
        };
        let text = |field: &Entity<InputState>| field.read(cx).value().to_string();
        let advanced = &fields.advanced;
        let mut form = fields.form.clone();
        form.typed.count = text(&fields.count);
        form.typed.times = fields.times.iter().map(|row| text(&row.input)).collect();
        form.typed.advanced = Advanced {
            every: text(&advanced.every),
            on: text(&advanced.on),
            at: text(&advanced.at),
            months: text(&advanced.months),
            days: text(&advanced.days),
            cron: text(&advanced.cron),
            jitter: text(&advanced.jitter),
        };
        Some(form)
    }

    pub fn action_form(&self, cx: &gpui_kit::App) -> Option<ActionForm> {
        let Fields::Action {
            form,
            username,
            save_to,
        } = &self.fields
        else {
            return None;
        };
        Some(form.typed(
            username.read(cx).value().to_string(),
            save_to.read(cx).value().to_string(),
        ))
    }

    pub fn webhook_form(&self, cx: &gpui_kit::App) -> Option<WebhookForm> {
        let Fields::Webhook {
            url,
            format,
            heartbeat,
            headers,
        } = &self.fields
        else {
            return None;
        };
        Some(WebhookForm {
            url: url.read(cx).value().to_string(),
            format: *format,
            heartbeat: *heartbeat,
            headers: headers
                .iter()
                .map(|row| {
                    (
                        row.name.read(cx).value().to_string(),
                        row.value.read(cx).value().to_string(),
                    )
                })
                .collect(),
        })
    }

    /// The step the dialog would make of what is typed, with the moment
    /// and the reader an action needs.
    pub fn step(
        &self,
        before: &Step,
        in_use: Option<snob_core::Pk>,
        now: snob_core::Epoch,
        cx: &gpui_kit::App,
    ) -> Option<Step> {
        if let Some(form) = self.schedule_form(cx) {
            return form.spec().ok().map(|when| Step::Schedule { when });
        }
        if let Some(form) = self.action_form(cx) {
            let account = match before {
                Step::Read { account, .. } => account.as_ref(),
                _ => None,
            };
            return Some(form.step(account, in_use, now));
        }
        if let Fields::Filter { only } = &self.fields {
            return Some(Step::Filter { only: *only });
        }
        self.webhook_form(cx).map(|form| form.step())
    }
}

/// A time of day to type, with `text` in it.
fn time_row(text: &str, look: &Look, window: &mut Window, cx: &mut Context<Shell>) -> TimeRow {
    let (input, watching) = field(text, look.tr("schedule-time-hint"), window, cx);
    TimeRow {
        input,
        _watching: watching,
    }
}

/// The fields of a schedule's dialog, holding what `form` has typed, and
/// the field focus goes to first: the one its frequency starts with.
fn schedule_fields(
    form: ScheduleForm,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> (ScheduleFields, Entity<InputState>) {
    use crate::state::schedule::{DayMode, Frequency};
    let mut watching = Vec::new();
    let mut text =
        |text: &str, placeholder: String, window: &mut Window, cx: &mut Context<Shell>| {
            let (input, sub) = field(text, placeholder, window, cx);
            watching.push(sub);
            input
        };
    let typed = form.typed.clone();
    let typed_in = &typed.advanced;
    let count = text(&typed.count, String::new(), window, cx);
    let advanced = AdvancedFields {
        every: text(&typed_in.every, look.tr("schedule-every-hint"), window, cx),
        on: text(&typed_in.on, look.tr("schedule-on-hint"), window, cx),
        at: text(&typed_in.at, look.tr("schedule-at-hint"), window, cx),
        months: text(
            &typed_in.months,
            look.tr("schedule-months-hint"),
            window,
            cx,
        ),
        days: text(&typed_in.days, look.tr("schedule-days-hint"), window, cx),
        cron: text(&typed_in.cron, look.tr("schedule-cron-hint"), window, cx),
        jitter: text(
            &typed_in.jitter,
            look.tr("schedule-jitter-hint"),
            window,
            cx,
        ),
    };
    let times: Vec<TimeRow> = typed
        .times
        .iter()
        .map(|time| time_row(time, look, window, cx))
        .collect();
    let choices = &form.choices;
    let first = match choices.frequency {
        Frequency::Advanced => advanced.every.clone(),
        Frequency::Week => count.clone(),
        Frequency::Day if choices.day_mode == DayMode::Repeating => count.clone(),
        Frequency::Day | Frequency::Month | Frequency::Year => times
            .first()
            .map_or_else(|| count.clone(), |row| row.input.clone()),
    };
    let fields = ScheduleFields {
        form,
        count,
        times,
        advanced,
        first_month: Select::new(12, cx),
        _watching: watching,
    };
    (fields, first)
}

/// A text field that redraws the window as it is typed in.
fn field(
    text: &str,
    placeholder: String,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> (Entity<InputState>, Subscription) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(text.to_string())
    });
    let watching = cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
        if let InputEvent::Change = event {
            cx.notify();
        }
    });
    (input, watching)
}

impl Shell {
    pub(crate) fn edit_mut(&mut self) -> Option<&mut Edit> {
        self.automations.as_mut()?.edit.as_mut()
    }

    /// The editor, open on `automation`, a canvas at its own size.
    pub(crate) fn open_the_editor(
        &mut self,
        automation: Automation,
        new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.automations.is_none() {
            return;
        }
        let has_a_trigger = crate::state::shelf::has_a_trigger(&automation);
        let name = {
            let typed = automation.name.clone();
            cx.new(|cx| InputState::new(window, cx).default_value(typed))
        };
        let naming = cx.subscribe_in(&name, window, |shell, input, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                let typed = input.read(cx).value().to_string();
                shell.rename_the_automation(&typed, cx);
            }
        });
        if let Some(page) = &mut self.automations {
            page.edit = Some(Edit {
                state: crate::state::editing::Editing::open(automation, new),
                name,
                selected: None,
                grab: None,
                wire: None,
                said: None,
                menu: None,
                dialog: None,
                leaving: None,
                _naming: naming,
            });
            page.editor = super::automations::Editor::new(has_a_trigger, cx);
        }
        window.focus(&self.title, cx);
        // Fitted once it has been drawn, which is what tells how big the
        // canvas is.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(40))
                .await;
            this.update(cx, |this, cx| this.fit_the_canvas_if_untouched(cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// Asks the backend to keep the automation the editor holds.
    pub(crate) fn keep_the_copy(&mut self, cx: &mut Context<Self>) {
        let Some(copy) = self
            .automations
            .as_ref()
            .and_then(|page| page.edit.as_ref())
            .map(|edit| edit.state.automation().clone())
        else {
            return;
        };
        self.keep(copy, cx);
    }

    /// Asks to keep `copy`.
    fn keep(&mut self, copy: Automation, cx: &mut Context<Self>) {
        self.order(Order::Keep(copy), cx);
    }

    /// Asks the backend for `order`: sent when nothing is on its way, else
    /// after what is.
    pub(crate) fn order(&mut self, order: Order, cx: &mut Context<Self>) {
        if let Some(order) = self.automation_saves.ask(order) {
            self.send(order, cx);
        }
    }

    fn send(&mut self, order: Order, cx: &mut Context<Self>) {
        let backend = self.backend().clone();
        let (id, kept, removed, judging, copy) = match order {
            Order::Keep(copy) => (
                copy.id.clone(),
                Some(backend.save_automation(copy.clone())),
                None,
                Some(backend.automation_problems(copy.clone())),
                Some(copy),
            ),
            Order::Forget(id) => (
                id.clone(),
                None,
                Some(backend.remove_automation(id)),
                None,
                None,
            ),
        };
        cx.spawn(async move |this, cx| {
            let kept = match kept {
                Some(kept) => Some(kept.await.unwrap_or_else(|_| Err(String::new()))),
                None => None,
            };
            let gone = match removed {
                Some(removed) => removed.await.is_ok(),
                None => false,
            };
            let judged = match judging {
                Some(judging) => judging.await.unwrap_or_default(),
                None => Vec::new(),
            };
            this.update(cx, |this, cx| {
                if let Some(page) = &mut this.automations {
                    if gone {
                        page.automations.retain(|one| one.id != id);
                    }
                    match (&kept, copy) {
                        (Some(Ok(())), Some(copy)) => {
                            match page.automations.iter_mut().find(|one| one.id == copy.id) {
                                Some(one) => *one = copy.clone(),
                                None => page.automations.push(copy.clone()),
                            }
                            if let Some(edit) = &mut page.edit {
                                if matches!(edit.said, Some(Said::NotKept(_))) {
                                    edit.said = None;
                                }
                                edit.state.judged(copy, judged);
                            }
                        }
                        (Some(Err(why)), _) => {
                            if let Some(edit) = &mut page.edit {
                                edit.said = Some(Said::NotKept(why.clone()));
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(next) = this.automation_saves.done() {
                    this.send(next, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Does `change` to the automation, and keeps it when it changed.
    fn change_the_automation(
        &mut self,
        change: impl FnOnce(&mut crate::state::editing::Editing) -> bool,
        cx: &mut Context<Self>,
    ) {
        let Some(edit) = self.edit_mut() else {
            return;
        };
        edit.said = None;
        if change(&mut edit.state) {
            self.keep_the_copy(cx);
        }
        cx.notify();
    }

    /// What was typed in the name's field.
    fn rename_the_automation(&mut self, typed: &str, cx: &mut Context<Self>) {
        self.change_the_automation(|state| state.rename(typed), cx);
    }

    /// Puts the field's text back to the automation's name, after an undo.
    fn show_the_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.edit_mut() else {
            return;
        };
        let (name, input) = (edit.state.automation().name.clone(), edit.name.clone());
        if input.read(cx).value() != name {
            input.update(cx, |input, cx| input.set_value(name, window, cx));
        }
    }

    pub(crate) fn undo_the_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.undo_or_redo(true, window, cx);
    }

    pub(crate) fn redo_the_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.undo_or_redo(false, window, cx);
    }

    fn undo_or_redo(&mut self, undo: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.edit_mut() else {
            return;
        };
        if edit.dialog.is_some() || edit.leaving.is_some() {
            return;
        }
        let done = if undo {
            edit.state.undo()
        } else {
            edit.state.redo()
        };
        if !done {
            return;
        }
        edit.said = None;
        edit.menu = None;
        let present = edit.state.automation().nodes.iter().map(|node| node.id);
        let gone = edit
            .selected
            .is_some_and(|id| !present.clone().any(|there| there == id));
        if gone {
            edit.selected = None;
        }
        tracing::info!(
            action = if undo {
                "automation_undo"
            } else {
                "automation_redo"
            },
            "action"
        );
        self.show_the_name(window, cx);
        self.keep_the_copy(cx);
        cx.notify();
    }

    /// A press on an item of the panel: its step is put on the canvas. The
    /// first trigger lets the panel go: it shows the kinds of step, the
    /// first of them focused.
    pub(crate) fn insert_a_step(
        &mut self,
        item: Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let in_use = self.active().map(|account| account.named.pk);
        let Some(step) = item.step(in_use) else {
            return;
        };
        let now = cx.background_executor().now();
        let Some(page) = &mut self.automations else {
            return;
        };
        let Some(edit) = &mut page.edit else {
            return;
        };
        let at = suggested_place(edit.state.automation(), step.stage());
        edit.said = None;
        edit.selected = edit.state.insert(step, at);
        let has_a_trigger = crate::state::shelf::has_a_trigger(edit.state.automation());
        let editor = &mut page.editor;
        if editor.shelf.follow(has_a_trigger) {
            editor.shelf_since = Some(now);
            editor.search = None;
            window.focus(&editor.kinds[0], cx);
        }
        tracing::info!(action = "automation_insert", "action");
        self.keep_the_copy(cx);
        cx.notify();
    }
}

impl Shell {
    /// A press on a step: it is chosen, and moves with the pointer until
    /// the button is let go.
    pub(crate) fn grab_a_step(&mut self, id: NodeId, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(page) = &mut self.automations else {
            return;
        };
        let world = page.editor.under(at, cx);
        let Some(edit) = &mut page.edit else {
            return;
        };
        let Some(node) = edit.state.automation().node(id) else {
            return;
        };
        if edit.wire.is_some() {
            return;
        }
        edit.selected = Some(id);
        edit.said = None;
        edit.grab = Some(Grab {
            id,
            offset: (world.0 - node.at.0 as f32, world.1 - node.at.1 as f32),
            from: node.at,
            at: node.at,
        });
        cx.notify();
    }

    /// A press on a step's output handle: an arrow starts there.
    pub(crate) fn begin_a_wire(&mut self, id: NodeId, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(page) = &mut self.automations else {
            return;
        };
        let world = page.editor.under(at, cx);
        if let Some(edit) = &mut page.edit {
            edit.said = None;
            edit.selected = Some(id);
            edit.wire = Some(Wire {
                from: id,
                to: world,
                snapped: None,
            });
            cx.notify();
        }
    }

    /// The small menu of a step, where the press was, or under the step
    /// when a key opened it.
    pub(crate) fn open_the_node_menu(
        &mut self,
        id: NodeId,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        if let Some(edit) = self.edit_mut() {
            edit.selected = Some(id);
            edit.grab = None;
            edit.wire = None;
            edit.menu = Some(NodeMenu { id, at, focus });
            cx.notify();
        }
    }

    /// The menu of a step opened by a key: under the step.
    pub(crate) fn open_the_node_menu_below(
        &mut self,
        id: NodeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = &self.automations else {
            return;
        };
        let Some(node) = page
            .edit
            .as_ref()
            .and_then(|edit| edit.state.automation().node(id))
        else {
            return;
        };
        let canvas = page.editor.canvas_here(cx);
        let (x, y) = canvas.on_screen((
            node.at.0 as f32,
            node.at.1 as f32 + crate::state::automations::NODE_H,
        ));
        let at = page.editor.middle() + gpui_kit::point(gpui_kit::px(x), gpui_kit::px(y * 1.0));
        self.open_the_node_menu(id, at, window, cx);
    }

    /// Whether the space bar is held over the canvas, which makes a drag
    /// move the canvas rather than a step.
    pub(crate) fn space_is_held(&self) -> bool {
        self.automations
            .as_ref()
            .is_some_and(|page| page.editor.space)
    }

    pub(crate) fn close_the_node_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self
            .automations
            .as_ref()
            .map(|page| page.editor.focus.clone());
        if let Some(edit) = self.edit_mut()
            && edit.menu.take().is_some()
        {
            if let Some(focus) = focus {
                window.focus(&focus, cx);
            }
            cx.notify();
        }
    }

    /// "Disable" or "Enable" of the step the menu is about.
    pub(crate) fn enable_the_step(
        &mut self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self
            .edit_mut()
            .and_then(|edit| edit.menu.as_ref().map(|menu| menu.id))
        else {
            return;
        };
        self.close_the_node_menu(window, cx);
        self.change_the_automation(|state| state.set_enabled(id, enabled), cx);
    }

    /// "Remove" of the step the menu is about.
    pub(crate) fn remove_the_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self
            .edit_mut()
            .and_then(|edit| edit.menu.as_ref().map(|menu| menu.id))
        else {
            return;
        };
        self.close_the_node_menu(window, cx);
        if let Some(edit) = self.edit_mut()
            && edit.selected == Some(id)
        {
            edit.selected = None;
        }
        self.change_the_automation(|state| state.remove(id), cx);
    }

    /// An arrow from the step the menu is about to `to`, or one taken off.
    pub(crate) fn join_the_steps(
        &mut self,
        to: NodeId,
        join: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(from) = self
            .edit_mut()
            .and_then(|edit| edit.menu.as_ref().map(|menu| menu.id))
        else {
            return;
        };
        self.close_the_node_menu(window, cx);
        let Some(edit) = self.edit_mut() else {
            return;
        };
        edit.said = None;
        let saves = if join {
            match edit.state.connect(from, to) {
                Ok(()) => true,
                Err(why) => {
                    edit.said = Some(Said::Unjoined(why));
                    false
                }
            }
        } else {
            edit.state.disconnect(from, to)
        };
        if saves {
            self.keep_the_copy(cx);
        }
        cx.notify();
    }
}

impl Shell {
    /// "Configure" of the step the menu is about: its dialog, over the
    /// window, on what the step holds now.
    pub(crate) fn open_the_configuration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self
            .edit_mut()
            .and_then(|edit| edit.menu.as_ref().map(|menu| menu.id))
        else {
            return;
        };
        self.close_the_node_menu(window, cx);
        let look = Look::now(cx).clone();
        let Some(step) = self.edit_mut().and_then(|edit| {
            edit.state
                .automation()
                .node(id)
                .map(|node| node.step.clone())
        }) else {
            return;
        };
        let mut watching = Vec::new();
        let mut text =
            |text: &str, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
                let (input, sub) = field(text, look.tr(placeholder), window, cx);
                watching.push(sub);
                input
            };
        let (fields, first) = match &step {
            Step::Schedule { when } => {
                let (fields, first) = schedule_fields(ScheduleForm::of(when), &look, window, cx);
                (Fields::Schedule(Box::new(fields)), Some(first))
            }
            Step::Read { account, what } => {
                let form = ActionForm::of(account.as_ref(), what);
                let username = text(&form.username, "action-username-hint", window, cx);
                let save_to = text(&form.save_to, "action-folder-hint", window, cx);
                (
                    Fields::Action {
                        form,
                        username,
                        save_to,
                    },
                    None,
                )
            }
            Step::Webhook(webhook) => {
                let form = WebhookForm::of(webhook);
                let url = text(&form.url, "webhook-url-hint", window, cx);
                let headers = form
                    .headers
                    .iter()
                    .map(|(name, value)| header_row(name, value, &look, window, cx))
                    .collect();
                let fields = Fields::Webhook {
                    url: url.clone(),
                    format: form.format,
                    heartbeat: form.heartbeat,
                    headers,
                };
                (fields, Some(url))
            }
            Step::Filter { only } => (Fields::Filter { only: *only }, None),
            // Nothing to say of it: it runs when somebody runs it.
            Step::Manual {} => return,
        };
        let (focus, cancel) = (cx.focus_handle(), cx.focus_handle());
        match first {
            Some(first) => first.update(cx, |input, cx| input.focus(window, cx)),
            None => window.focus(&cancel, cx),
        }
        if let Some(edit) = self.edit_mut() {
            edit.dialog = Some(Dialog {
                node: id,
                fields,
                focus,
                cancel,
                _watching: watching,
            });
        }
        cx.notify();
    }

    /// "Save" of the dialog: what it holds is the step now.
    pub(crate) fn apply_the_configuration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let in_use = self.active().map(|account| account.named.pk);
        let now = snob_core::clock::now();
        let Some(edit) = self.edit_mut() else {
            return;
        };
        let Some(dialog) = &edit.dialog else {
            return;
        };
        let id = dialog.node;
        let Some(before) = edit
            .state
            .automation()
            .node(id)
            .map(|node| node.step.clone())
        else {
            return;
        };
        let Some(step) = dialog.step(&before, in_use, now, cx) else {
            return;
        };
        tracing::info!(action = "automation_configure", "action");
        self.close_the_configuration(window, cx);
        self.change_the_automation(|state| state.configure(id, step), cx);
    }

    pub(crate) fn close_the_configuration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self
            .automations
            .as_ref()
            .map(|page| page.editor.focus.clone());
        if let Some(edit) = self.edit_mut()
            && edit.dialog.take().is_some()
        {
            if let Some(focus) = focus {
                window.focus(&focus, cx);
            }
            cx.notify();
        }
    }
}

impl Shell {
    fn fields_mut(&mut self) -> Option<&mut Fields> {
        self.edit_mut()?
            .dialog
            .as_mut()
            .map(|dialog| &mut dialog.fields)
    }

    fn schedule_mut(&mut self) -> Option<&mut ScheduleFields> {
        match self.fields_mut()? {
            Fields::Schedule(fields) => Some(fields),
            _ => None,
        }
    }

    /// A choice of a schedule's dialog changed.
    pub(crate) fn change_the_schedule(
        &mut self,
        change: impl FnOnce(&mut Choices),
        cx: &mut Context<Self>,
    ) {
        if let Some(fields) = self.schedule_mut() {
            change(&mut fields.form.choices);
            cx.notify();
        }
    }

    /// How often a schedule repeats. Advanced's fields are filled from the
    /// choices when they make a schedule (`ScheduleForm::choose`).
    pub(crate) fn choose_the_frequency(
        &mut self,
        frequency: crate::state::schedule::Frequency,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut form) = self
            .edit_mut()
            .and_then(|edit| edit.dialog.as_ref())
            .and_then(|dialog| dialog.schedule_form(cx))
        else {
            return;
        };
        form.choose(frequency);
        let Some(fields) = self.schedule_mut() else {
            return;
        };
        let typed = &form.typed.advanced;
        let wanted = [
            &typed.every,
            &typed.on,
            &typed.at,
            &typed.months,
            &typed.days,
            &typed.cron,
            &typed.jitter,
        ];
        let inputs: Vec<(Entity<InputState>, String)> = fields
            .advanced
            .all()
            .into_iter()
            .zip(wanted)
            .map(|(input, text)| (input.clone(), text.clone()))
            .collect();
        fields.form = form;
        for (input, text) in inputs {
            if input.read(cx).value() != text {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
        cx.notify();
    }

    /// One more or one less in a schedule's count, never under one.
    pub(crate) fn step_the_count(&mut self, by: i32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fields) = self.schedule_mut() else {
            return;
        };
        let input = fields.count.clone();
        let now = input.read(cx).value().trim().parse::<u64>().unwrap_or(1);
        let next = now
            .saturating_add_signed(i64::from(by))
            .clamp(1, MOST_COUNT)
            .to_string();
        input.update(cx, |input, cx| input.set_value(next, window, cx));
        cx.notify();
    }

    /// "Add a time": an empty time of day, focused.
    pub(crate) fn add_a_time(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let look = Look::now(cx).clone();
        let row = time_row("", &look, window, cx);
        row.input.update(cx, |input, cx| input.focus(window, cx));
        if let Some(fields) = self.schedule_mut() {
            fields.times.push(row);
            cx.notify();
        }
    }

    pub(crate) fn remove_a_time(&mut self, at: usize, cx: &mut Context<Self>) {
        if let Some(fields) = self.schedule_mut()
            && at < fields.times.len()
        {
            fields.times.remove(at);
            cx.notify();
        }
    }

    /// Opens or closes the months every few months may start from, focus
    /// on the one chosen while they are open.
    pub(crate) fn toggle_the_first_months(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fields) = self.schedule_mut() else {
            return;
        };
        let select = &mut fields.first_month;
        select.open = !select.open;
        let chosen = fields.form.choices.first_month.clamp(1, 12) as usize - 1;
        let focus = if select.open {
            select.options.get(chosen).cloned()
        } else {
            Some(select.field.clone())
        };
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(crate) fn choose_the_first_month(
        &mut self,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(fields) = self.schedule_mut() {
            fields.form.choices.first_month = at as u32 + 1;
        }
        self.close_the_first_months(window, cx);
    }

    /// Closes the months every few months may start from, focus back on
    /// the field. Whether they were open.
    pub(crate) fn close_the_first_months(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(fields) = self.schedule_mut() else {
            return false;
        };
        if !fields.first_month.open {
            return false;
        }
        fields.first_month.open = false;
        let field = fields.first_month.field.clone();
        window.focus(&field, cx);
        cx.notify();
        true
    }

    /// Whose account an action reads: the viewer's own, or another's.
    pub(crate) fn read_as_mine(&mut self, mine: bool, cx: &mut Context<Self>) {
        if let Some(Fields::Action { form, .. }) = self.fields_mut() {
            form.mine = mine;
            cx.notify();
        }
    }

    /// The yes to walking another account's lists, ticked or not.
    pub(crate) fn consent_to_the_lists(&mut self, consent: bool, cx: &mut Context<Self>) {
        if let Some(Fields::Action { form, .. }) = self.fields_mut() {
            form.consent = consent;
            cx.notify();
        }
    }

    pub(crate) fn choose_the_quality(
        &mut self,
        quality: Option<VideoQuality>,
        cx: &mut Context<Self>,
    ) {
        if let Some(Fields::Action { form, .. }) = self.fields_mut() {
            form.quality = quality;
            cx.notify();
        }
    }

    pub(crate) fn choose_the_format(&mut self, format: WebhookFormat, cx: &mut Context<Self>) {
        if let Some(Fields::Webhook { format: chosen, .. }) = self.fields_mut() {
            *chosen = format;
            cx.notify();
        }
    }

    pub(crate) fn choose_the_filter(&mut self, only: Only, cx: &mut Context<Self>) {
        if let Some(Fields::Filter { only: chosen }) = self.fields_mut() {
            *chosen = only;
            cx.notify();
        }
    }

    pub(crate) fn toggle_the_heartbeat(&mut self, cx: &mut Context<Self>) {
        if let Some(Fields::Webhook { heartbeat, .. }) = self.fields_mut() {
            *heartbeat = !*heartbeat;
            cx.notify();
        }
    }

    pub(crate) fn add_a_header(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let look = Look::now(cx).clone();
        let row = header_row("", "", &look, window, cx);
        row.name.update(cx, |input, cx| input.focus(window, cx));
        if let Some(Fields::Webhook { headers, .. }) = self.fields_mut() {
            headers.push(row);
            cx.notify();
        }
    }

    pub(crate) fn remove_a_header(&mut self, at: usize, cx: &mut Context<Self>) {
        if let Some(Fields::Webhook { headers, .. }) = self.fields_mut()
            && at < headers.len()
        {
            headers.remove(at);
            cx.notify();
        }
    }

    /// "Choose": the folder files are saved in, from the system's picker.
    pub(crate) fn choose_the_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Fields::Action { save_to, .. }) = self.fields_mut() else {
            return;
        };
        let save_to = save_to.clone();
        let choosing = self.system.choose_folder(cx);
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Some(folder)) = choosing.await else {
                return;
            };
            let shown = folder.display().to_string();
            cx.update(|window, cx| {
                save_to.update(cx, |input, cx| input.set_value(shown, window, cx));
            })
            .ok();
        })
        .detach();
    }
}

/// A row of headers, with the name and the value typed in.
fn header_row(
    name: &str,
    value: &str,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> HeaderRow {
    let (name, one) = field(name, look.tr("webhook-header-name-hint"), window, cx);
    let (value, other) = field(value, look.tr("webhook-header-value-hint"), window, cx);
    HeaderRow {
        name,
        value,
        _watching: [one, other],
    }
}

impl Shell {
    /// The way back from the editor (its button, Escape): at once when
    /// what is left can run, else after the question.
    pub(crate) fn leave_the_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.try_to_leave(None, window, cx);
    }

    /// Leaves, or asks. `then` is where the person was going.
    fn try_to_leave(&mut self, then: Option<Event>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.edit_mut() else {
            return;
        };
        let kept = edit.state.automation().clone();
        match edit.state.leaving() {
            Leaving::Quietly => self.finish_leaving(Some(kept), then, window, cx),
            Leaving::Forget(id) => {
                self.order(Order::Forget(id), cx);
                self.finish_leaving(None, then, window, cx);
            }
            Leaving::Ask(problems) => {
                let (focus, cancel) = (cx.focus_handle(), cx.focus_handle());
                window.focus(&cancel, cx);
                if let Some(edit) = self.edit_mut() {
                    edit.menu = None;
                    edit.dialog = None;
                    edit.leaving = Some(Leave {
                        problems,
                        then,
                        focus,
                        cancel,
                    });
                }
                cx.notify();
            }
        }
    }

    /// The editor closes: the page of `kept` when there is one, else the
    /// list; and the way out goes on to `then`.
    fn finish_leaving(
        &mut self,
        kept: Option<Automation>,
        then: Option<Event>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = &mut self.automations else {
            return;
        };
        let Some(edit) = page.edit.take() else {
            return;
        };
        let id = edit.state.automation().id.clone();
        page.editor = super::automations::Editor::new(true, cx);
        match &kept {
            Some(_) if page.open.as_ref().is_some_and(|open| open.id == id) => {}
            Some(_) => {
                page.open = Some(Opened::new(id.clone()));
            }
            None => page.open = None,
        }
        if let Some(copy) = &kept {
            match page.automations.iter_mut().find(|one| one.id == id) {
                Some(one) => *one = copy.clone(),
                None => page.automations.push(copy.clone()),
            }
        }
        window.focus(&self.title, cx);
        if kept.is_some() {
            self.read_the_history(id, kept, window, cx);
        }
        cx.notify();
        if let Some(event) = then {
            self.happened(event, window, cx);
        }
    }

    /// "Discard changes": what the editor opened with is put back, a new
    /// automation is removed, and the way out goes on.
    pub(crate) fn discard_the_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.edit_mut() else {
            return;
        };
        let then = edit.leaving.take().and_then(|leave| leave.then);
        let discarding = edit.state.discarding();
        tracing::info!(action = "automation_discard", "action");
        let kept = match discarding {
            Discard::Remove(id) => {
                self.order(Order::Forget(id), cx);
                None
            }
            Discard::Restore(before) => {
                self.order(Order::Keep(before.clone()), cx);
                Some(before)
            }
        };
        self.finish_leaving(kept, then, window, cx);
    }

    /// "Keep editing": the question is dropped, and so is where the person
    /// was going.
    pub(crate) fn keep_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self
            .automations
            .as_ref()
            .map(|page| page.editor.focus.clone());
        if let Some(edit) = self.edit_mut()
            && edit.leaving.take().is_some()
        {
            if let Some(focus) = focus {
                window.focus(&focus, cx);
            }
            cx.notify();
        }
    }

    /// Holds `event` while the editor holds an automation that cannot run:
    /// the question comes first, and `event` goes on after a discard.
    pub(crate) fn guard_the_editor(
        &mut self,
        event: &Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let leaves = matches!(
            event,
            Event::Go(_)
                | Event::OpenSettings
                | Event::AddAccount
                | Event::OpenProfile(_)
                | Event::OpenList { .. }
        );
        if !leaves || !self.is_editing() {
            return false;
        }
        let Some(edit) = self.edit_mut() else {
            return false;
        };
        match edit.state.leaving() {
            Leaving::Quietly => false,
            Leaving::Forget(id) => {
                self.order(Order::Forget(id), cx);
                false
            }
            Leaving::Ask(_) => {
                self.try_to_leave(Some(event.clone()), window, cx);
                true
            }
        }
    }

    /// Closes what is open over the editor, the innermost first: the menu,
    /// the dialog, the question. Whether something was.
    pub(crate) fn dismiss_an_editor_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(edit) = self.edit_mut() else {
            return false;
        };
        let (leaving, dialog, menu) = (
            edit.leaving.is_some(),
            edit.dialog.is_some(),
            edit.menu.is_some(),
        );
        if leaving {
            self.keep_editing(window, cx);
        } else if dialog {
            // Escape closes the open month list before the dialog.
            if !self.close_the_first_months(window, cx) {
                self.close_the_configuration(window, cx);
            }
        } else if menu {
            self.close_the_node_menu(window, cx);
        } else {
            return false;
        }
        true
    }
}
