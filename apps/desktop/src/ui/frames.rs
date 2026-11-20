//! Every frame a view asks for, asked here ([`ask`]), so the shell can tell
//! the meter (`state::frames`) a frame something moving asked for from one
//! a pointer, a key or an answer drew. **Nothing else in the app calls
//! `request_animation_frame`**: a test reads the sources.

use std::cell::RefCell;

use gpui_kit::Window;

thread_local! {
    /// The windows, by id, whose next frame has been asked for.
    static ASKED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// Asks for `window`'s next frame, for something that moves.
pub fn ask(window: &Window) {
    note(window.window_handle().window_id().as_u64());
    window.request_animation_frame();
}

/// Whether `window`'s frame before this one asked for it, forgotten once
/// said: read before anything of this frame asks.
pub fn asked(window: &Window) -> bool {
    taken(window.window_handle().window_id().as_u64())
}

fn note(id: u64) {
    ASKED.with_borrow_mut(|asked| {
        if !asked.contains(&id) {
            asked.push(id);
        }
    });
}

fn taken(id: u64) -> bool {
    ASKED.with_borrow_mut(|asked| match asked.iter().position(|each| *each == id) {
        Some(at) => {
            asked.swap_remove(at);
            true
        }
        None => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_asked_for_is_said_once_and_for_its_own_window_alone() {
        note(7);
        note(7);
        note(9);
        assert!(taken(7));
        assert!(!taken(7), "said once");
        assert!(!taken(8), "another window asked nothing");
        assert!(taken(9));
    }

    #[test]
    fn nothing_but_this_module_asks_for_a_frame() {
        fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|ext| ext == "rs")
                    && !path.ends_with("ui/frames.rs")
                    && std::fs::read_to_string(&path)
                        .unwrap()
                        .contains("request_animation_frame")
                {
                    found.push(path.display().to_string());
                }
            }
        }
        let mut found = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut found,
        );
        assert!(found.is_empty(), "ask through `ui::frames::ask`: {found:?}");
    }
}
