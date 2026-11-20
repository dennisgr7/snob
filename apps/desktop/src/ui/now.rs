//! The one moment the words of a screen are said against ("Today at
//! 12:18", "Yesterday"): this computer's clock, read where a view is
//! drawn, never kept. **A test build may be told the moment it is**
//! (`--fixed-now`), so a picture of a screen says the same whatever the
//! day it is taken; told one, it also holds still what is read again by
//! itself (`state::automations::reads_again`).

use gpui_kit::App;
#[cfg(feature = "testing")]
use gpui_kit::Global;
use snob_core::Epoch;

/// The moment it is, for the words of a screen.
pub fn of(cx: &App) -> Epoch {
    #[cfg(feature = "testing")]
    if let Some(fixed) = cx.try_global::<FixedNow>() {
        return fixed.0;
    }
    let _ = cx;
    snob_core::clock::now()
}

/// Whether a test build was told the moment it is: never in a release.
pub fn is_fixed(cx: &App) -> bool {
    #[cfg(feature = "testing")]
    if cx.has_global::<FixedNow>() {
        return true;
    }
    let _ = cx;
    false
}

/// The moment a test build was told it is.
#[cfg(feature = "testing")]
struct FixedNow(Epoch);

#[cfg(feature = "testing")]
impl Global for FixedNow {}

/// From now on the words of every screen are said against `at`.
#[cfg(feature = "testing")]
pub fn fix_now(at: Epoch, cx: &mut App) {
    cx.set_global(FixedNow(at));
}
