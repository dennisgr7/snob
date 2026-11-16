//! The app's own drawings, built in: the icons a status, a button or a
//! caption button shows. Each is a mask a view colors by role, never a
//! picture with colors of its own.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($name:ident = $file:literal;)*) => {
        $(pub const $name: &str = concat!("icons/", $file);)*

        const ALL: &[(&str, &[u8])] = &[
            $((
                concat!("icons/", $file),
                include_bytes!(concat!("../../assets/icons/", $file)).as_slice(),
            ),)*
        ];
    };
}

icons! {
    OFFLINE = "offline.svg";
    PAUSED = "paused.svg";
    RESUMING = "resuming.svg";
    DOWNLOADING = "downloading.svg";
    UPDATE = "update.svg";
    ERROR = "error.svg";
    SHOW = "show.svg";
    HIDE = "hide.svg";
    CHEVRON = "chevron.svg";
    BACK = "back.svg";
    ADD = "add.svg";
    SETTINGS = "settings.svg";
    HOME = "home.svg";
    SEARCH = "search.svg";
    AUTOMATIONS = "automations.svg";
    SIGN_OUT = "sign-out.svg";
    WINDOW_MINIMIZE = "window-minimize.svg";
    WINDOW_MAXIMIZE = "window-maximize.svg";
    WINDOW_RESTORE = "window-restore.svg";
    WINDOW_CLOSE = "window-close.svg";
    STEP_SITE = "step-site.svg";
    STEP_TOOLS = "step-tools.svg";
    STEP_COOKIES = "step-cookies.svg";
    STEP_COPY = "step-copy.svg";
    BROWSER = "browser.svg";
    BROWSER_CHROME = "browser-chrome.svg";
    BROWSER_EDGE = "browser-edge.svg";
    BROWSER_BRAVE = "browser-brave.svg";
    PROFILE = "profile.svg";
    GRID = "grid.svg";
    LIST = "list.svg";
    LAYOUT_STACKED = "layout-stacked.svg";
    LAYOUT_SPLIT = "layout-split.svg";
    MORE = "more.svg";
    DOWNLOAD = "download.svg";
    COPY = "copy.svg";
    CLOSE = "close.svg";
    NEW_WINDOW = "new-window.svg";
    CAROUSEL = "carousel.svg";
    VIDEO = "video.svg";
    PHOTO = "photo.svg";
    REPOST = "repost.svg";
    MENTION = "mention.svg";
}

/// The drawing of the browser the engine calls `name`: its own mark, drawn
/// plain, or a browser like any other.
pub fn browser(name: &str) -> &'static str {
    match name {
        "Chrome" => BROWSER_CHROME,
        "Edge" => BROWSER_EDGE,
        "Brave" => BROWSER_BRAVE,
        _ => BROWSER,
    }
}

/// What GPUI loads a drawing from.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ALL
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ALL
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_drawing_named_is_built_in_and_is_a_drawing() {
        assert_eq!(ALL.len(), 43);
        for (name, bytes) in ALL {
            let loaded = Assets.load(name).unwrap().expect("built in");
            assert_eq!(&*loaded, *bytes);
            assert!(bytes.starts_with(b"<svg "), "{name}");
        }
        assert!(Assets.load("icons/none.svg").unwrap().is_none());
    }
}
