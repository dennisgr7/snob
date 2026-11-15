//! The terminal guard every interactive view draws through.
//!
//! Everything is drawn on **standard error**.
//!
//! The browsers take the **alternate screen**, so leaving one takes the last
//! frame with it. What the user came for must therefore be said again on the
//! way out: each browser collects receipt lines -- `Saved ./someone-3.jpg`,
//! `Opened https://...` -- and hands them to [`print_receipts`] after its guard
//! has dropped, which puts them on the real screen where the shell prompt
//! lands.

use std::borrow::Cow;
use std::io::{Stderr, Write, stderr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use console::measure_text_width;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, HighlightSpacing, Padding, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table,
};
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::ui::graphics;
use crate::ui::theme::Role;
use snob_engine::app::Viewer;

/// DEC private mode 2026, "synchronized output": between the two the terminal
/// holds what it is sent and paints it in one go.
const BEGIN_UPDATE: &[u8] = b"\x1b[?2026h";
const END_UPDATE: &[u8] = b"\x1b[?2026l";
const HIDE_CURSOR: &[u8] = b"\x1b[?25l";
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";

/// What a frame in which no cell changed still asks to have written: the
/// backend ends every frame by resetting the foreground, the background, the
/// underline's color and the attributes (`ratatui-crossterm` 0.1.2,
/// `Backend::draw`, with its `underline-color` feature, which is on), and the
/// terminal then hides the cursor (`ratatui-core` 0.1.2,
/// `apply_buffer_with_cursor`).
const UNCHANGED_FRAME: &[u8] = b"\x1b[39m\x1b[49m\x1b[59m\x1b[0m\x1b[?25l";

/// The same frame where `NO_COLOR` is set: crossterm then writes no color at
/// all, the reset of one included, and what is left of each of the three is an
/// SGR with no parameter (crossterm 0.29.0, `Colored`'s `Display`, which reads
/// the variable once).
const UNCHANGED_FRAME_NO_COLOR: &[u8] = b"\x1b[m\x1b[m\x1b[m\x1b[0m\x1b[?25l";

/// Whether these bytes are all of a frame in which no cell changed.
fn says_nothing(frame: &[u8]) -> bool {
    frame == UNCHANGED_FRAME || frame == UNCHANGED_FRAME_NO_COLOR
}

/// Where the frames are written: standard error, a whole frame at a time.
///
/// Here a frame is collected and leaves in **one write, inside one synchronized
/// update**, so it is painted whole or not yet.
pub struct Screen<W: Write = Stderr> {
    out: W,
    pending: Vec<u8>,
    /// Whether the last thing written about the cursor hid it.
    cursor_hidden: Arc<AtomicBool>,
    /// The pictures a frame placed over its cells, which go out after the
    /// frame's own bytes, in the same update (`ui::graphics`).
    painter: graphics::Painter,
}

impl<W: Write> Screen<W> {
    fn new(out: W) -> Self {
        Self {
            out,
            pending: Vec::new(),
            cursor_hidden: Arc::new(AtomicBool::new(false)),
            painter: graphics::Painter::new(),
        }
    }
}

/// What every frame carries and nothing else a view writes does: the reset of
/// the attributes the backend ends a frame's cells with.
const FRAME_END: &[u8] = b"\x1b[0m";

fn is_a_frame(bytes: &[u8]) -> bool {
    bytes
        .windows(FRAME_END.len())
        .any(|window| window == FRAME_END)
}

impl<W: Write> Write for Screen<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let unchanged = self.cursor_hidden.load(Ordering::Relaxed) && says_nothing(&self.pending);
        // The frame's pictures, after its cells: a frame that wrote no cell
        // may still have a picture to show, one that arrived meanwhile.
        let mut pictures = Vec::new();
        if is_a_frame(&self.pending) {
            self.painter.after_frame(!unchanged, &mut pictures);
        } else {
            self.painter.after_anything_else(&mut pictures);
        }
        if unchanged && pictures.is_empty() {
            self.pending.clear();
            return Ok(());
        }
        if self.pending.ends_with(HIDE_CURSOR) {
            self.cursor_hidden.store(true, Ordering::Relaxed);
        } else if self.pending.ends_with(SHOW_CURSOR) {
            self.cursor_hidden.store(false, Ordering::Relaxed);
        }
        let mut update = Vec::with_capacity(
            BEGIN_UPDATE.len() + self.pending.len() + pictures.len() + END_UPDATE.len(),
        );
        update.extend_from_slice(BEGIN_UPDATE);
        update.append(&mut self.pending);
        update.append(&mut pictures);
        update.extend_from_slice(END_UPDATE);
        self.out.write_all(&update)?;
        self.out.flush()
    }
}

/// The claim on the terminal, and the `ratatui` terminal that draws under it.
pub struct Tui {
    pub terminal: Terminal<CrosstermBackend<Screen>>,
    /// The screen's own note of whether the cursor is hidden, cleared when
    /// a suspension has shown it behind the screen's back.
    cursor_hidden: Arc<AtomicBool>,
    /// Whether this guard took the alternate screen, so `suspend` knows
    /// whether there is one to leave.
    fullscreen: bool,
    /// False while suspended, so `Drop` after `suspend` undoes nothing twice
    /// and a failed `resume` does not leave `Drop` believing modes are on.
    active: bool,
}

impl Tui {
    /// Raw mode, the alternate screen, bracketed paste, focus reporting, cursor
    /// hidden.
    pub fn fullscreen() -> std::io::Result<Self> {
        Self::enter(true, Viewport::Fullscreen)
    }

    /// The same claim without the alternate screen, drawing in `height` rows
    /// at the cursor. For the menu, which lives between a wizard's questions.
    pub fn inline(height: u16) -> std::io::Result<Self> {
        Self::enter(false, Viewport::Inline(height))
    }

    fn enter(fullscreen: bool, viewport: Viewport) -> std::io::Result<Self> {
        enable_raw_mode()?;
        let modes = if fullscreen {
            crossterm::execute!(
                stderr(),
                EnterAlternateScreen,
                EnableBracketedPaste,
                EnableFocusChange,
                Hide
            )
        } else {
            crossterm::execute!(stderr(), EnableBracketedPaste, EnableFocusChange, Hide)
        };
        let screen = Screen::new(stderr());
        let cursor_hidden = screen.cursor_hidden.clone();
        let terminal = modes.and_then(|()| {
            Terminal::with_options(CrosstermBackend::new(screen), TerminalOptions { viewport })
        });
        match terminal {
            Ok(terminal) => Ok(Self {
                terminal,
                cursor_hidden,
                fullscreen,
                active: true,
            }),
            // Raw mode is already on and half the modes may be too; undo the
            // whole claim rather than leave a terminal nobody owns.
            Err(e) => {
                crate::ui::restore_terminal();
                Err(e)
            }
        }
    }

    /// Hands the terminal back -- cooked, real screen, cursor shown -- so
    /// somebody else may write to it.
    pub fn suspend(&mut self) -> std::io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let modes = if self.fullscreen {
            crossterm::execute!(
                stderr(),
                DisableFocusChange,
                DisableBracketedPaste,
                LeaveAlternateScreen,
                Show
            )
        } else {
            crossterm::execute!(stderr(), DisableFocusChange, DisableBracketedPaste, Show)
        };
        let raw = disable_raw_mode();
        // Back to a command in a terminal, which somebody waits on, whatever
        // the view's focus last said (`owner::attended`).
        snob_engine::owner::attended(true);
        crate::ui::browser::input::focus_unknown();
        modes.and(raw)?;
        self.active = false;
        Ok(())
    }

    /// Takes the claim back and repaints from nothing: whatever was drawn
    /// while suspended, the renderer's belief about the screen is wrong now.
    pub fn resume(&mut self) -> std::io::Result<()> {
        if self.active {
            return Ok(());
        }
        enable_raw_mode()?;
        let modes = if self.fullscreen {
            crossterm::execute!(
                stderr(),
                EnterAlternateScreen,
                EnableBracketedPaste,
                EnableFocusChange,
                Hide
            )
        } else {
            crossterm::execute!(stderr(), EnableBracketedPaste, EnableFocusChange, Hide)
        };
        if let Err(e) = modes {
            // Raw mode is on and half the modes may be too.
            crate::ui::restore_terminal();
            return Err(e);
        }
        self.active = true;
        // The suspension showed the cursor behind the screen's back.
        self.cursor_hidden.store(false, Ordering::Relaxed);
        self.terminal.clear()
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.suspend();
    }
}

/// The whole entry, shared by every browser: the terminal check and the claim,
/// each refusal carrying the view's own hint about the flags that work without
/// one.
pub fn claim_fullscreen(hint: &str) -> anyhow::Result<Tui> {
    use snob_engine::exit::{ExitCode, ExitError};

    if !console::Term::stderr().is_term() {
        return Err(
            ExitError::new(ExitCode::Error, "--interactive needs a terminal to draw on")
                .with_hint(hint)
                .into(),
        );
    }
    // The whole claim on the terminal -- raw mode, alternate screen, paste,
    // cursor -- in one guard, so an early return cannot leave any of it on.
    Ok(Tui::fullscreen().map_err(|e| {
        ExitError::new(
            ExitCode::Error,
            format!("the terminal would not go into raw mode: {e}"),
        )
        .with_hint(hint)
    })?)
}

/// The chrome's two modifiers: dim for asides, bold for what a row is about.
pub const DIM: Style = Style::new().add_modifier(Modifier::DIM);
pub const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);

/// `style` where the views may style, no style at all where they may not.
pub fn paint(colors: bool, style: Style) -> Style {
    if colors { style } else { Style::new() }
}

/// A line in `style`, or the plain text.
pub fn styled_line<'a>(text: impl Into<Cow<'a, str>>, style: Style, colors: bool) -> Line<'a> {
    if colors {
        Line::from(Span::styled(text, style))
    } else {
        Line::raw(text)
    }
}

/// Whether the views may style at all.
pub fn colors_enabled() -> bool {
    console::Term::stderr().features().colors_supported()
}

/// Says on the real screen what the alternate screen took away.
pub fn print_receipts(lines: &[String]) {
    let term = console::Term::stderr();
    for line in lines {
        let _ = term.write_line(line);
    }
}

/// The frame every view sits in: rounded corners, the view's name bold on the
/// top edge, and the breathing room its content asked for.
pub fn view_block(title: String, colors: bool, padding: Padding) -> Block<'static> {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .padding(padding);
    if colors {
        block.border_style(DIM).title_top(title_line(&title))
    } else {
        block.title_top(Line::from(format!(" {title} ")))
    }
}

/// A view's title as it is styled: bold, and what it is about in the accent.
fn title_line(title: &str) -> Line<'static> {
    let cut = [" · ", " — "]
        .iter()
        .filter_map(|separator| title.find(separator))
        .min()
        .unwrap_or(title.len());
    let (about, rest) = title.split_at(cut);
    Line::from(vec![
        Span::styled(format!(" {about}"), accent_bold()),
        Span::styled(format!("{rest} "), BOLD),
    ])
}

/// The accent, bold: a title's word, the handle on a card, a key in the help.
pub fn accent_bold() -> Style {
    Role::Accent.style().add_modifier(Modifier::BOLD)
}

/// The breathing room a list gets: one blank row under the border, one cell on
/// the left so rows do not touch the frame.
pub fn list_padding(area: Rect) -> Padding {
    if area.height < 10 {
        Padding::ZERO
    } else {
        Padding::new(1, 1, 1, 0)
    }
}

/// The card's padding: `proportional` because a terminal cell is twice as
/// tall as it is wide, so equal visual margins are 2 columns and 1 row.
pub fn card_padding() -> Padding {
    Padding::proportional(1)
}

/// Where the selection sits in a list that does not fit, for the top-right of
/// the border.
pub fn position_line(selected: usize, total: usize, colors: bool) -> Line<'static> {
    styled_line(format!(" {}/{total} ", selected + 1), DIM, colors).right_aligned()
}

/// The account a view acts as, dim, for the top-right of the border.
pub fn viewer_line(viewer: &Viewer, colors: bool) -> Line<'static> {
    styled_line(format!(" as {} ", viewer.label()), DIM, colors).right_aligned()
}

/// The top-right of a view's border: the position when there is one, then
/// the account in the corner. ratatui lays right-aligned titles out in the
/// order they were added, the last one in the corner and the first one
/// cut short when the row runs out, so the account is added last.
pub fn top_right<'a>(block: Block<'a>, position: Option<Line<'a>>, viewer: Line<'a>) -> Block<'a> {
    match position {
        Some(position) => block.title_top(position),
        None => block,
    }
    .title_top(viewer)
}

/// The key hints, dim on the bottom edge.
pub fn hint_line(text: String, colors: bool) -> Line<'static> {
    styled_line(format!(" {text} "), DIM, colors)
}

/// Something worth a look -- `Nothing walked`, `No more comments` -- in the
/// hints' place.
pub fn note_line(text: String, colors: bool) -> Line<'static> {
    styled_line(format!(" {text} "), Role::Warn.style(), colors)
}

/// What was done as asked -- `Saved ./x.jpg`, `Opened ...` -- in the hints'
/// place, in the green the site's terminals say "ok" in: a receipt is not a
/// warning, and yellow was saying both.
pub fn done_line(text: String, colors: bool) -> Line<'static> {
    styled_line(format!(" {text} "), Role::Ok.style(), colors)
}

/// What went wrong -- `Could not open it: ...` -- in the hints' place.
pub fn problem_line(text: String, colors: bool) -> Line<'static> {
    styled_line(format!(" {text} "), Role::Error.style(), colors)
}

/// The right line for what a verb reported: a failure is a problem, a receipt
/// is done, the rest are notes.
pub fn outcome_line(text: String, colors: bool) -> Line<'static> {
    if text.starts_with("Could not") {
        problem_line(text, colors)
    } else if text.starts_with("Saved ") || text.starts_with("Opened ") {
        done_line(text, colors)
    } else {
        note_line(text, colors)
    }
}

/// The bottom edge: the note when there is one, the hints otherwise.
pub fn footer(note: &str, hint: &str, colors: bool) -> Line<'static> {
    if note.is_empty() {
        hint_line(hint.to_string(), colors)
    } else {
        outcome_line(note.to_string(), colors)
    }
}

/// The note for a file handed to the system viewer.
pub fn opened_note(result: anyhow::Result<PathBuf>) -> String {
    match result {
        Ok(path) => format!("Opened {}", path.display()),
        Err(e) => format!("Could not open it: {e}"),
    }
}

/// The note for a file saved where the user works, and its receipt.
pub fn saved_note(result: anyhow::Result<PathBuf>, receipts: &mut Vec<String>) -> String {
    match result {
        Ok(path) => {
            let line = format!("Saved {}", path.display());
            receipts.push(line.clone());
            line
        }
        Err(e) => format!("Could not save it: {e}"),
    }
}

/// The public page of an account: what `w` hands to the system's browser.
pub fn page_of(username: &str) -> String {
    format!(
        "https://www.instagram.com/{}/",
        snob_core::model::in_a_path(username)
    )
}

/// Hands `url` to whatever the system opens addresses with, and says so: a note
/// and a receipt, for [`saved_note`]'s reason.
pub fn web_note(url: &str, receipts: &mut Vec<String>) -> String {
    match opener::open(url) {
        Ok(()) => {
            let line = format!("Opened {url}");
            receipts.push(line.clone());
            line
        }
        Err(e) => format!("Could not open it: {e}"),
    }
}

/// The scroll position, drawn over the right border.
pub fn scrollbar(frame: &mut Frame<'_>, outer: Rect, total: usize, selected: usize, viewport: u16) {
    let mut state = ScrollbarState::new(total)
        .position(selected)
        .viewport_content_length(viewport as usize);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None),
        // Inside the corners, so ╮ and ╯ survive.
        outer.inner(Margin {
            horizontal: 0,
            vertical: 1,
        }),
        &mut state,
    );
}

/// The one selection style: reverse video, never a color.
pub fn selection() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// A table's header row: dim and underlined, no color and no blank row after it
/// -- the underline already separates, and a spare blank row on a list of a
/// hundred accounts is a row of data lost.
pub fn header_row(labels: &[&str], colors: bool) -> Row<'static> {
    Row::new(
        labels
            .iter()
            .map(|l| Cell::from((*l).to_string()))
            .collect::<Vec<_>>(),
    )
    .style(paint(colors, DIM.add_modifier(Modifier::UNDERLINED)))
}

/// A list's table: two columns between columns, the selection in reverse
/// video, and a `> ` beside it -- what is left of the selection when there
/// is no styling at all.
pub fn list_table<'a, R, C>(rows: R, widths: C) -> Table<'a>
where
    R: IntoIterator,
    R::Item: Into<Row<'a>>,
    C: IntoIterator,
    C::Item: Into<Constraint>,
{
    Table::new(rows, widths)
        .column_spacing(2)
        .row_highlight_style(selection())
        .highlight_symbol("> ")
        .highlight_spacing(HighlightSpacing::Always)
}

/// Where a modal lands: the optical center, one third from the top rather than
/// the geometric middle.
pub fn modal_area(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let [_, band, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(2),
    ])
    .areas(area);
    band.centered_horizontally(Constraint::Length(width))
}

/// Dims everything drawn so far, so the modal on top of it reads as on top.
pub fn veil(frame: &mut Frame<'_>, colors: bool) {
    // A picture the terminal draws itself is not a cell to dim, and would
    // stand bright over the box: it is taken back, and returns with the
    // first frame that has no box.
    graphics::hide();
    if colors {
        let area = frame.area();
        frame.buffer_mut().set_style(area, DIM);
    }
}

/// Clears the modal's cells and draws its frame, handing back the interior.
pub fn open_modal(frame: &mut Frame<'_>, area: Rect, block: Block<'_>) -> Rect {
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    inner
}

/// The lines `text` takes at `width` display columns: its own line breaks kept,
/// an empty one included, and each line wrapped greedily at the blanks.
pub fn wrapped(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        let mut used = 0usize;
        for word in paragraph.split_whitespace() {
            let wide = measure_text_width(word);
            if used > 0 && used + 1 + wide <= width {
                line.push(' ');
                line.push_str(word);
                used += 1 + wide;
                continue;
            }
            if used > 0 {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            if wide <= width {
                line.push_str(word);
                used = wide;
                continue;
            }
            for letter in word.chars() {
                let cells = measure_text_width(letter.encode_utf8(&mut [0; 4]));
                if used > 0 && used + cells > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push(letter);
                used += cells;
            }
        }
        lines.push(line);
    }
    lines
}

/// What `List::scroll_padding` does and `Table` does not: keep `padding` rows
/// visible on each side of the selection.
pub fn scrolled_offset(
    offset: usize,
    selected: usize,
    total: usize,
    viewport: usize,
    padding: usize,
) -> usize {
    if viewport == 0 || total == 0 {
        return 0;
    }
    let pad = padding.min(viewport.saturating_sub(1) / 2);
    let max = total.saturating_sub(viewport);
    let top = selected.saturating_sub(pad);
    let bottom = (selected + pad).min(total - 1);
    offset
        .min(max)
        .min(top)
        .max(bottom.saturating_sub(viewport.saturating_sub(1)))
        .min(max)
}

/// One row of a test terminal, its whole width, as drawn.
#[cfg(test)]
pub(crate) fn row_text(terminal: &Terminal<ratatui::backend::TestBackend>, y: u16) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

/// The account the views' tests act as: `@me`.
#[cfg(test)]
pub(crate) fn me() -> Viewer {
    Viewer {
        pk: 7.into(),
        username: Some("me".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer that keeps each write apart, as a terminal receives them.
    #[derive(Default)]
    struct Writes(Vec<Vec<u8>>);

    impl Write for Writes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// One frame as the backend hands it over: in pieces, then a flush.
    fn frame(screen: &mut Screen<Writes>, pieces: &[&[u8]]) {
        for piece in pieces {
            screen.write_all(piece).unwrap();
        }
        screen.flush().unwrap();
    }

    /// However many pieces a frame arrives in, the terminal gets it in one
    /// write, between the two ends of a synchronized update.
    #[test]
    fn a_frame_is_one_synchronized_update_in_one_write() {
        let mut screen = Screen::new(Writes::default());
        frame(&mut screen, &[b"\x1b[1;1H", b"a", b"b", HIDE_CURSOR]);
        assert_eq!(
            screen.out.0,
            [b"\x1b[?2026h\x1b[1;1Hab\x1b[?25l\x1b[?2026l".to_vec()]
        );
    }

    /// The frame an idle view draws seven times a second sends no byte, and
    /// a flush with nothing collected sends no empty update.
    #[test]
    fn an_unchanged_frame_writes_nothing() {
        // As crossterm spells it, and as it does where `NO_COLOR` is set.
        for unchanged in [UNCHANGED_FRAME, UNCHANGED_FRAME_NO_COLOR] {
            let mut screen = Screen::new(Writes::default());
            frame(&mut screen, &[b"x", unchanged]);
            assert_eq!(screen.out.0.len(), 1, "the first frame is written");
            for _ in 0..3 {
                frame(&mut screen, &[unchanged]);
                screen.flush().unwrap();
            }
            assert_eq!(screen.out.0.len(), 1, "nothing after it");
        }
    }

    /// After a frame that showed the cursor, or after somebody else had the
    /// terminal, the same bytes are the ones that hide it: they are written.
    #[test]
    fn a_cursor_left_showing_is_hidden_by_the_next_frame() {
        let mut screen = Screen::new(Writes::default());
        frame(&mut screen, &[UNCHANGED_FRAME]);
        assert_eq!(screen.out.0.len(), 1, "nothing says it is hidden yet");

        frame(&mut screen, &[b"x", SHOW_CURSOR]);
        frame(&mut screen, &[b"\x1b[2;4H"]);
        frame(&mut screen, &[UNCHANGED_FRAME]);
        assert_eq!(screen.out.0.len(), 4, "the frame after a shown cursor");
        frame(&mut screen, &[UNCHANGED_FRAME]);
        assert_eq!(screen.out.0.len(), 4, "and then it is unchanged");

        screen.cursor_hidden.store(false, Ordering::Relaxed);
        frame(&mut screen, &[UNCHANGED_FRAME]);
        assert_eq!(screen.out.0.len(), 5, "the frame after a suspension");
    }

    /// crossterm's `NO_COLOR` switch is one for the process, read from the
    /// environment once, and two tests here throw it: one at a time.
    static COLOR_SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Runs `check` with crossterm writing colors and then writing none,
    /// whatever the environment of the test run says, and leaves the switch
    /// as it was found. `check` is told whether colors are off.
    fn with_colors_and_without(mut check: impl FnMut(bool)) {
        use crossterm::style::Colored;

        let _alone = COLOR_SWITCH
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let found = Colored::ansi_color_disabled_memoized();
        for off in [false, true] {
            Colored::set_ansi_color_disabled(off);
            check(off);
        }
        Colored::set_ansi_color_disabled(found);
    }

    /// The bytes dropped are the ones crossterm writes for what a frame with
    /// no changed cell still sends, with `NO_COLOR` and without it, and the
    /// cursor's two are crossterm's.
    #[test]
    fn the_unchanged_frame_is_what_crossterm_writes_for_it() {
        use crossterm::Command;
        use crossterm::style::{Attribute, Color, SetAttribute, SetUnderlineColor};
        use crossterm::style::{SetBackgroundColor, SetForegroundColor};

        with_colors_and_without(|off| {
            let mut written = String::new();
            SetForegroundColor(Color::Reset)
                .write_ansi(&mut written)
                .unwrap();
            SetBackgroundColor(Color::Reset)
                .write_ansi(&mut written)
                .unwrap();
            SetUnderlineColor(Color::Reset)
                .write_ansi(&mut written)
                .unwrap();
            SetAttribute(Attribute::Reset)
                .write_ansi(&mut written)
                .unwrap();
            Hide.write_ansi(&mut written).unwrap();
            let unchanged = if off {
                UNCHANGED_FRAME_NO_COLOR
            } else {
                UNCHANGED_FRAME
            };
            assert_eq!(written.as_bytes(), unchanged, "colors off: {off}");
        });

        let mut hide = String::new();
        Hide.write_ansi(&mut hide).unwrap();
        assert_eq!(hide.as_bytes(), HIDE_CURSOR);
        let mut show = String::new();
        Show.write_ansi(&mut show).unwrap();
        assert_eq!(show.as_bytes(), SHOW_CURSOR);
    }

    /// The renderer itself, through the backend the views use: the second of
    /// two frames that are the same reaches the terminal as nothing, with
    /// `NO_COLOR` and without it. This is what holds [`UNCHANGED_FRAME`] and
    /// its twin to the ratatui in the tree, where the test above holds them
    /// only to crossterm.
    #[test]
    fn the_renderer_writes_nothing_for_a_frame_it_already_drew() {
        with_colors_and_without(the_same_frame_again_is_no_write);
    }

    fn the_same_frame_again_is_no_write(off: bool) {
        /// Keeps the writes that get past the screen, each apart.
        #[derive(Clone, Default)]
        struct Kept(Arc<std::sync::Mutex<Vec<String>>>);

        impl Write for Kept {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let text = String::from_utf8_lossy(bytes).into_owned();
                self.0.lock().unwrap().push(text);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let out = Kept::default();
        let writes = || out.0.lock().unwrap().len();
        // A fixed viewport, so nothing asks a terminal for its size.
        let made = Terminal::with_options(
            CrosstermBackend::new(Screen::new(out.clone())),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 20, 3)),
            },
        );
        let Ok(mut terminal) = made else { return };
        let mut draw = || {
            terminal
                .draw(|frame| frame.render_widget("hello", frame.area()))
                .is_ok()
        };
        // On a Windows console that takes no escape sequences crossterm
        // draws through the console's own calls, which a test has no console
        // for: nothing passes through the screen there, and nothing is held.
        if !draw() || writes() == 0 {
            return;
        }
        assert_eq!(writes(), 1, "the first frame, in one write");
        assert!(draw() && draw(), "the same frame twice more");
        // Copied out first: the terminal writes once more as it is dropped,
        // and must not find the lock held by a failing assertion.
        let all = out.0.lock().unwrap().clone();
        assert_eq!(
            all.len(),
            1,
            "and no write for either, colors off {off}: {all:?}"
        );
    }

    /// The address `w` opens is the public page, the name encoded, and the
    /// very one the printed table puts behind a username.
    #[test]
    fn the_browser_gets_the_public_page() {
        assert_eq!(page_of("someone"), "https://www.instagram.com/someone/");
        assert_eq!(page_of("a b"), "https://www.instagram.com/a%20b/");
        for name in ["someone", "a b", "søren", "x\x1b]8;;http://evil.test"] {
            let user = snob_core::model::User {
                pk: 1.into(),
                username: name.to_string(),
                full_name: None,
                is_private: None,
                is_verified: None,
                pfp_url: None,
            };
            assert_eq!(page_of(name), user.profile_url(), "{name:?}");
        }
    }

    /// Walking the selection from end to end keeps two rows of margin
    /// visible wherever the list allows it, and the offset never overshoots.
    #[test]
    fn the_offset_keeps_the_margin_and_never_overshoots() {
        let (total, viewport, pad) = (50usize, 10usize, 2usize);
        let mut offset = 0usize;
        for selected in 0..total {
            offset = scrolled_offset(offset, selected, total, viewport, pad);
            assert!(offset <= total - viewport, "offset {offset} past the end");
            assert!(
                selected >= offset && selected < offset + viewport,
                "selection {selected} fell out of [{offset}, {})",
                offset + viewport
            );
            if selected >= pad && selected < total - pad {
                assert!(
                    selected >= offset + pad && selected <= offset + viewport - 1 - pad,
                    "margin lost at {selected} with offset {offset}"
                );
            }
        }
    }

    /// Without styling a style is dropped whole, a role's with the rest;
    /// with it, kept as given.
    #[test]
    fn paint_keeps_a_style_only_where_the_views_may_style() {
        let accent = accent_bold();
        assert_eq!(paint(true, accent), accent);
        assert_eq!(paint(false, accent), Style::new());
        assert_eq!(paint(false, DIM), Style::new());
        for role in [Role::Accent, Role::Ok, Role::Warn, Role::Error, Role::Info] {
            assert_eq!(paint(false, role.style()), Style::new(), "{role:?}");
        }
    }

    /// A receipt is green, a failure red and anything else the warning's
    /// yellow, by the first word the views write.
    #[test]
    fn a_receipt_a_failure_and_a_note_are_three_colors() {
        let color = |text: &str| outcome_line(text.to_string(), true).spans[0].style;
        assert_eq!(color("Saved ./someone-1.jpg"), Role::Ok.style());
        assert_eq!(color("Opened ./someone-1.jpg"), Role::Ok.style());
        assert_eq!(color("Could not save it: disk full"), Role::Error.style());
        assert_eq!(color("Nothing walked"), Role::Warn.style());
        assert_eq!(color("Saved"), Role::Warn.style(), "a word, not a receipt");
        assert!(
            outcome_line("Saved ./x.jpg".to_string(), false)
                .spans
                .iter()
                .all(|span| span.style == Style::new())
        );
    }

    /// A title's name is in the accent and the rest of it only bold; the
    /// text is the title's, a space each side, whatever the styling.
    #[test]
    fn a_title_says_what_it_is_about_in_the_accent() {
        let line = title_line("Stories · @someone · 3 up");
        assert_eq!(line.to_string(), " Stories · @someone · 3 up ");
        assert_eq!(line.spans[0].content, " Stories");
        assert_eq!(line.spans[0].style, accent_bold());
        assert_eq!(line.spans[1].style, BOLD);

        let line = title_line("followers of @someone — 120 accounts");
        assert_eq!(line.spans[0].content, " followers of @someone");

        let line = title_line("People");
        assert_eq!(line.to_string(), " People ");
        assert_eq!(line.spans[0].style, accent_bold());
    }

    /// The notes the verbs leave: what was opened or saved, or why not, with
    /// a receipt only for what was saved.
    #[test]
    fn the_notes_say_what_happened_and_only_a_save_leaves_a_receipt() {
        let path = || Ok(PathBuf::from("./someone-1.jpg"));
        let failed = || Err(anyhow::anyhow!("the viewer would not start"));
        assert_eq!(opened_note(path()), "Opened ./someone-1.jpg");
        assert_eq!(
            opened_note(failed()),
            "Could not open it: the viewer would not start"
        );

        let mut receipts = Vec::new();
        assert_eq!(saved_note(path(), &mut receipts), "Saved ./someone-1.jpg");
        assert_eq!(receipts, ["Saved ./someone-1.jpg"]);
        assert_eq!(
            saved_note(failed(), &mut receipts),
            "Could not save it: the viewer would not start"
        );
        assert_eq!(receipts.len(), 1, "a failure leaves no receipt");

        // Only the top context reaches the note, as `{e}` gives it.
        let layered = Err(anyhow::anyhow!("disk full").context("could not write x"));
        assert_eq!(opened_note(layered), "Could not open it: could not write x");
    }

    /// A viewport too small for the margin still shows the selection.
    #[test]
    fn a_tiny_viewport_still_follows_the_selection() {
        let mut offset = 0usize;
        for selected in 0..10 {
            offset = scrolled_offset(offset, selected, 10, 3, 2);
            assert!(selected >= offset && selected < offset + 3);
        }
    }

    fn top_row(block: Block<'_>, width: u16) -> String {
        let mut terminal =
            Terminal::new(ratatui::backend::TestBackend::new(width, 3)).expect("a test terminal");
        terminal
            .draw(|frame| frame.render_widget(block, frame.area()))
            .expect("a frame");
        row_text(&terminal, 0)
    }

    /// The account sits in the corner and the position to its left.
    #[test]
    fn the_account_takes_the_corner_after_the_position() {
        let block = top_right(
            view_block("People".to_string(), false, Padding::ZERO),
            Some(position_line(2, 40, false)),
            viewer_line(&me(), false),
        );
        let row = top_row(block, 40);
        assert!(row.ends_with(" 3/40 ─ as @me ╮"), "got {row:?}");
        assert!(row.starts_with("╭ People "), "got {row:?}");
    }

    /// Without a position the account alone is in the corner, and an account
    /// whose name is not known yet is named by its id.
    #[test]
    fn the_account_alone_takes_the_corner() {
        let nameless = Viewer {
            pk: 7.into(),
            username: None,
        };
        let block = top_right(
            view_block("People".to_string(), false, Padding::ZERO),
            None,
            viewer_line(&nameless, false),
        );
        let row = top_row(block, 40);
        assert!(row.ends_with("─ as account 7 ╮"), "got {row:?}");
    }

    /// On a row too narrow for both, the account keeps its place and the
    /// position is what gets cut.
    #[test]
    fn a_narrow_row_keeps_the_account() {
        let block = top_right(
            Block::bordered(),
            Some(position_line(2, 40, false)),
            viewer_line(&me(), false),
        );
        let row = top_row(block, 12);
        assert!(row.ends_with(" as @me ┐"), "got {row:?}");
    }

    /// A text is wrapped at the blanks, within the width, and keeps the line
    /// breaks its writer put in it, the empty ones too.
    #[test]
    fn a_text_wraps_at_the_blanks_and_keeps_its_own_line_breaks() {
        assert_eq!(
            wrapped("one two three four", 9),
            ["one two", "three", "four"]
        );
        assert_eq!(
            wrapped("first\n\nsecond line", 20),
            ["first", "", "second line"]
        );
        assert_eq!(wrapped("  spaced   out  ", 20), ["spaced out"]); // layout-allow: the blanks are what is being collapsed
        assert!(wrapped("", 20).is_empty());
        for line in wrapped("a bio of some words and a few more of them", 7) {
            assert!(measure_text_width(&line) <= 7, "{line:?}");
        }
    }

    /// A word wider than the line is cut at the edge, in display columns:
    /// a wide character is never split and never pushed past the width.
    #[test]
    fn a_word_wider_than_the_line_is_cut_at_the_edge() {
        assert_eq!(
            wrapped("see example.com/a/long/path now", 10),
            ["see", "example.co", "m/a/long/p", "ath now"]
        );
        // Each of these is two columns wide: two fit in five, not three.
        assert_eq!(
            wrapped("日本語のテキスト", 5),
            ["日本", "語の", "テキ", "スト"]
        );
        // A width of nothing is taken as one, and still ends.
        assert_eq!(wrapped("ab", 0), ["a", "b"]);
    }

    /// The modal band sits above the geometric middle, centered across.
    #[test]
    fn the_modal_lands_on_the_optical_center() {
        let area = Rect::new(0, 0, 100, 30);
        let modal = modal_area(area, 46, 7);
        assert_eq!(modal.width, 46);
        assert_eq!(modal.height, 7);
        assert_eq!(modal.x, 27, "centered: (100 - 46) / 2");
        assert!(
            modal.y + modal.height / 2 < 15,
            "the middle of the modal is above the middle of the screen, got y {}",
            modal.y
        );
    }
}
