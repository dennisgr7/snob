//! A list's scrolling that knows when its thumb is held: the toolkit's
//! scrollbar (`gpui_base::Scrollbar`) is handed this handle, which says so
//! as the thumb is pressed and let go, and the settings show their outline
//! for as long as it is. **A press on the track that is no drag shows no
//! outline**: the toolkit calls neither end for it.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::ScrollbarHandle;
use gpui_kit::{Bounds, Pixels, Point, ScrollHandle, Size};

/// A [`ScrollHandle`], and whether its thumb is held.
#[derive(Clone, Default)]
pub struct OutlinedScroll {
    pub handle: ScrollHandle,
    dragging: Rc<Cell<bool>>,
}

impl OutlinedScroll {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the thumb is held now.
    pub fn dragging(&self) -> bool {
        self.dragging.get()
    }
}

impl ScrollbarHandle for OutlinedScroll {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.handle.bounds()
    }

    fn offset(&self) -> Point<Pixels> {
        self.handle.offset()
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.handle.set_offset(offset);
    }

    fn content_size(&self) -> Size<Pixels> {
        (self.handle.max_offset() + self.handle.bounds().size.into()).into()
    }

    fn start_drag(&self) {
        self.dragging.set(true);
    }

    fn end_drag(&self) {
        self.dragging.set(false);
    }
}

/// Where each of a list's parts reads on its scrollbar's track: the part's
/// top in the list as a share of the whole list, laid on a track
/// `track` tall, and which part is read now, the last whose top has
/// scrolled past. `tops` are each part's top inside the list, `offset` how
/// far it is scrolled, and `whole` how tall it is.
pub fn outline(tops: &[f32], offset: f32, whole: f32, track: f32) -> (Vec<f32>, Option<usize>) {
    if whole <= 0.0 {
        return (Vec::new(), None);
    }
    let on_track = tops
        .iter()
        .map(|top| (top / whole).clamp(0.0, 1.0) * track)
        .collect();
    // Half a line of slack, so the heading just reached already counts.
    let read = tops.iter().rposition(|top| *top <= offset + 12.0);
    (on_track, read.or((!tops.is_empty()).then_some(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_reads_on_the_track_where_its_share_of_the_list_is() {
        let (at, read) = outline(&[0.0, 600.0], 0.0, 1200.0, 400.0);
        assert_eq!(at, [0.0, 200.0]);
        assert_eq!(read, Some(0));
        let (_, read) = outline(&[0.0, 600.0], 600.0, 1200.0, 400.0);
        assert_eq!(read, Some(1), "scrolled to it, it is the one read");
        let (_, read) = outline(&[0.0, 600.0], 590.0, 1200.0, 400.0);
        assert_eq!(read, Some(1), "nearly there counts");
    }

    #[test]
    fn an_empty_list_has_no_outline() {
        assert_eq!(outline(&[], 0.0, 0.0, 400.0), (Vec::new(), None));
        assert_eq!(outline(&[], 0.0, 100.0, 400.0), (Vec::new(), None));
    }
}
