//! The parts the screens are built from, drawn alone in a window: what a
//! screen reader finds of each, by name and by role.

#![cfg(feature = "testing")]

use gpui_kit::test::TestQueryExt as _;
use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, Role, Styled as _, TestAppContext, Window,
    div,
};
use snob_desktop::system::Settings;
use snob_desktop::ui::look::Look;
use snob_desktop::ui::parts::{self, Tone};

struct Drawn {
    look: Look,
}

impl Render for Drawn {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let look = &self.look;
        div()
            .size_full()
            .flex()
            .flex_col()
            // Two rows of a list, each saying the same word.
            .child(parts::badge(
                "first-verified",
                "Verified",
                Tone::Verified,
                look,
            ))
            .child(parts::badge(
                "second-verified",
                "Verified",
                Tone::Verified,
                look,
            ))
            .child(parts::badge("first-private", "Private", Tone::Plain, look))
            .child(
                parts::chips("days", "On these days", look)
                    .child(parts::chip("day-mon", "Monday", "Mon", true, look))
                    .child(parts::chip("day-tue", "Tuesday", "Tue", false, look)),
            )
            .child(parts::checkbox(
                "consent",
                "Yes, read the lists",
                true,
                look,
            ))
    }
}

#[gpui_kit::test]
fn a_badge_a_chip_and_a_checkbox_say_their_name_role_and_state(cx: &mut TestAppContext) {
    let look = Look::of(Settings::default(), false);
    let (_, cx) = cx.add_window_view(move |_, _| Drawn { look });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        for (badge, drawn) in [("Verified", 2), ("Private", 1)] {
            let found = window.find_all_by_label(badge);
            assert_eq!(found.len(), drawn, "{badge}");
            for found in found {
                assert_eq!(found.role(), Some(Role::Label), "{badge}");
                assert!(found.visible(), "{badge}");
            }
        }

        let days = window.find_by_label("On these days");
        assert_eq!(days.role(), Some(Role::Group));
        for (name, on) in [("Monday", true), ("Tuesday", false)] {
            let chip = window.find_by_label(name);
            assert_eq!(chip.role(), Some(Role::CheckBox), "{name}");
            assert_eq!(chip.checked(), Some(on), "{name}");
            let bounds = chip.bounds();
            assert!(
                f32::from(bounds.size.width) >= 24.0 && f32::from(bounds.size.height) >= 24.0,
                "{name} is {bounds:?}"
            );
        }

        let consent = window.find_by_label("Yes, read the lists");
        assert_eq!(consent.role(), Some(Role::CheckBox));
        assert_eq!(consent.checked(), Some(true));
    });
}
