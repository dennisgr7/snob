//! The parts the screens are built from, drawn alone in a window: what a
//! screen reader finds of each, by name and by role, and where they are.

#![cfg(feature = "testing")]

use gpui_kit::test::{TestQueryExt as _, TestSupportExt as _, TestWindowExt as _};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Role,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, div, px,
};
use snob_desktop::system::Settings;
use snob_desktop::theme::tokens::shape;
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

/// The same field twice, each in a box that clips it, one of them given
/// room for a focus ring.
struct Clipped;

impl Render for Clipped {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let held = |id: &'static str, clip: &'static str, field: &'static str, room: bool| {
            let clips = div()
                .id(clip)
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .child(div().id(field).test_support().h(px(40.)));
            div()
                .id(id)
                .test_support()
                .h(px(120.))
                .flex()
                .flex_col()
                .child(if room {
                    parts::room_for_focus(clips)
                } else {
                    clips
                })
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(20.))
            .p(px(20.))
            .child(held("plain-holder", "plain-clip", "plain-field", false))
            .child(held("room-holder", "room-clip", "room-field", true))
    }
}

#[gpui_kit::test]
fn room_for_a_focus_ring_moves_nothing_and_leaves_the_ring_its_reach_on_every_side(
    cx: &mut TestAppContext,
) {
    let (_, cx) = cx.add_window_view(|_, _| Clipped);
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        let room = px(shape::FOCUS_ROOM);
        let (plain_holder, plain) = (
            window.find("plain-holder").bounds(),
            window.find("plain-field").bounds(),
        );
        let (holder, clip, field) = (
            window.find("room-holder").bounds(),
            window.find("room-clip").bounds(),
            window.find("room-field").bounds(),
        );

        // Where the field is in what holds it, and how big, as without room.
        assert_eq!(
            field.origin - holder.origin,
            plain.origin - plain_holder.origin
        );
        assert_eq!(field.size, plain.size);
        assert_eq!(window.find("plain-clip").bounds(), plain_holder);

        // What clips it reaches past it as far as a ring does.
        assert_eq!(field.left() - clip.left(), room);
        assert_eq!(clip.right() - field.right(), room);
        assert_eq!(field.top() - clip.top(), room);
        assert_eq!(clip.bottom() - holder.bottom(), room);
        assert_eq!(holder.top() - clip.top(), room);
    });
}
