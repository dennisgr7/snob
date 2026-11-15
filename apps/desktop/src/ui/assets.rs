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
    RING = "ring.svg";
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
    SIGN_OUT = "sign-out.svg";
    WINDOW_MINIMIZE = "window-minimize.svg";
    WINDOW_MAXIMIZE = "window-maximize.svg";
    WINDOW_RESTORE = "window-restore.svg";
    WINDOW_CLOSE = "window-close.svg";
    STEP_SITE = "step-site.svg";
    STEP_TOOLS = "step-tools.svg";
    STEP_COOKIES = "step-cookies.svg";
    STEP_COPY = "step-copy.svg";
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
        assert_eq!(ALL.len(), 22);
        for (name, bytes) in ALL {
            let loaded = Assets.load(name).unwrap().expect("built in");
            assert_eq!(&*loaded, *bytes);
            assert!(bytes.starts_with(b"<svg "), "{name}");
        }
        assert!(Assets.load("icons/none.svg").unwrap().is_none());
    }
}
