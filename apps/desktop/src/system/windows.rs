//! What Windows says and GPUI does not: a contrast theme, animations turned
//! off, transparency turned off or the battery saver on, the text size,
//! whether there is a network, and the session ending.
//!
//! **A window of its own, never shown, on a thread of its own**: a change
//! of settings and the end of a session are broadcasts, which a window
//! only for messages is not sent. **The network is what Windows says it
//! is**: nothing is sent to find out.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetNetworkConnectivityHint, NotifyNetworkConnectivityHintChange,
};
use windows_sys::Win32::Networking::WinSock::{
    NL_NETWORK_CONNECTIVITY_HINT, NL_NETWORK_CONNECTIVITY_LEVEL_HINT,
    NetworkConnectivityLevelHintConstrainedInternetAccess,
    NetworkConnectivityLevelHintInternetAccess, NetworkConnectivityLevelHintNone,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GWLP_USERDATA, GetMessageW,
    GetWindowLongPtrW, MSG, PBT_APMPOWERSTATUSCHANGE, PostMessageW, RegisterClassW,
    SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST, SetWindowLongPtrW, SystemParametersInfoW,
    WM_APP, WM_ENDSESSION, WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_SETTINGCHANGE,
    WM_THEMECHANGED, WNDCLASSW, WS_OVERLAPPED,
};

use super::{Event, Settings};

/// How long the end of a session waits for what holds it: inside the five
/// seconds or so Windows waits for an answer.
const PATIENCE: Duration = Duration::from_millis(4_500);

const CLASS: &str = "snob-desktop-system";

/// Sent to the window with the reason, a boxed wide string, for it to show
/// if the session's end is kept waiting.
const HOLD: u32 = WM_APP + 1;
/// Sent to the window when a hold is dropped.
const RELEASE: u32 = WM_APP + 2;

const NOT_SAID: u8 = 0;
const ONLINE: u8 = 1;
const OFFLINE: u8 = 2;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// A number the person's own registry holds.
fn stored(key: &str, value: &str) -> Option<u32> {
    let (key, value) = (wide(key), wide(value));
    let mut found = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: both names are NUL-terminated and outlive the call; the data
    // pointer is a `u32` the call may write `size` bytes to, and the flag
    // refuses a value of any other type.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut found).cast(),
            &raw mut size,
        )
    };
    (status == 0).then_some(found)
}

fn more_contrast() -> bool {
    let mut contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..HIGHCONTRASTW::default()
    };
    // SAFETY: the structure says its own size, which is also the size
    // handed over, and the call only writes to it.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            (&raw mut contrast).cast(),
            0,
        )
    };
    read != 0 && contrast.dwFlags & HCF_HIGHCONTRASTON != 0
}

fn reduce_motion() -> bool {
    let mut animated = 1i32;
    // SAFETY: this setting is read into a `BOOL`, which `animated` is.
    let read = unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animated).cast(), 0)
    };
    read != 0 && animated == 0
}

fn saving_battery() -> bool {
    let mut power = SYSTEM_POWER_STATUS::default();
    // SAFETY: the structure is the call's to fill.
    let read = unsafe { GetSystemPowerStatus(&raw mut power) };
    read != 0 && power.SystemStatusFlag == 1
}

/// What Windows is set to, laid over `settings`: the theme and the language
/// are GPUI's and the system's own to say.
fn read_into(settings: &mut Settings) {
    settings.more_contrast = more_contrast();
    settings.reduce_motion = reduce_motion();
    settings.reduce_transparency = saving_battery()
        || stored(
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "EnableTransparency",
        ) == Some(0);
    // Settings, Accessibility, Text size: a percentage, 100 to 225.
    settings.text_scale = stored(r"Software\Microsoft\Accessibility", "TextScaleFactor")
        .map_or(1.0, |percent| percent.clamp(100, 225) as f32 / 100.0);
}

/// **Only "no network at all" counts as offline.** "Local access" is what
/// Windows says behind a proxy or a portal it cannot see through, where
/// Instagram may well answer: taken as not said.
fn network(level: NL_NETWORK_CONNECTIVITY_LEVEL_HINT) -> u8 {
    #[allow(non_upper_case_globals)]
    match level {
        NetworkConnectivityLevelHintNone => OFFLINE,
        NetworkConnectivityLevelHintInternetAccess
        | NetworkConnectivityLevelHintConstrainedInternetAccess => ONLINE,
        _ => NOT_SAID,
    }
}

/// What the window and the network's callback share with the app.
struct Shared {
    settings: Arc<Mutex<Settings>>,
    said: Box<dyn Fn(Event) + Send + Sync>,
    network: AtomicU8,
    /// How many things ask the end of the session to wait.
    holds: Mutex<usize>,
    released: Condvar,
}

impl Shared {
    fn say(&self, event: Event) {
        (self.said)(event);
    }

    /// Reads the settings again and says them when one changed.
    fn settings_may_have_changed(&self) {
        let changed = {
            let mut settings = self.settings.lock().unwrap_or_else(|e| e.into_inner());
            let before = settings.clone();
            read_into(&mut settings);
            (*settings != before).then(|| settings.clone())
        };
        if let Some(settings) = changed {
            self.say(Event::Settings(settings));
        }
    }

    fn network_is(&self, level: NL_NETWORK_CONNECTIVITY_LEVEL_HINT) {
        let now = network(level);
        let before = self.network.swap(now, Ordering::Relaxed);
        // Going from not said to there being one changes nothing.
        if now != before && (now == OFFLINE || before == OFFLINE) {
            self.say(Event::Online(now != OFFLINE));
        }
    }
}

/// Windows, listened to for the life of the process.
pub(super) struct Windows {
    shared: Arc<Shared>,
    /// The window as a number, since a handle does not cross threads.
    window: Option<isize>,
}

impl Windows {
    /// Reads what Windows is set to into `settings`, and says every change
    /// from now on through `said`, from threads of Windows' own.
    pub(super) fn listen(
        settings: Arc<Mutex<Settings>>,
        said: impl Fn(Event) + Send + Sync + 'static,
    ) -> Self {
        read_into(&mut settings.lock().unwrap_or_else(|e| e.into_inner()));
        let shared = Arc::new(Shared {
            settings,
            said: Box::new(said),
            network: AtomicU8::new(NOT_SAID),
            holds: Mutex::new(0),
            released: Condvar::new(),
        });

        let mut hint = NL_NETWORK_CONNECTIVITY_HINT::default();
        // SAFETY: the structure is the call's to fill.
        if unsafe { GetNetworkConnectivityHint(&raw mut hint) } == 0 {
            shared
                .network
                .store(network(hint.ConnectivityLevel), Ordering::Relaxed);
        }
        // One reference for the callback, never given back: it is called
        // for as long as the process lives.
        let kept = Arc::into_raw(Arc::clone(&shared));
        let mut registration: HANDLE = std::ptr::null_mut();
        // SAFETY: the callback is `network_changed`, whose context is the
        // pointer kept alive above; the handle is the call's to write.
        let status = unsafe {
            NotifyNetworkConnectivityHintChange(
                Some(network_changed),
                kept.cast(),
                false,
                &raw mut registration,
            )
        };
        if status != 0 {
            tracing::debug!(status, "Windows will not say when the network changes");
        }

        let window = window(Arc::clone(&shared));
        Self { shared, window }
    }

    pub(super) fn online(&self) -> Option<bool> {
        match self.shared.network.load(Ordering::Relaxed) {
            ONLINE => Some(true),
            OFFLINE => Some(false),
            _ => None,
        }
    }

    /// Asks the end of a session to wait until the guard is dropped, for
    /// `PATIENCE` at most; `why` is what Windows shows if it is kept
    /// waiting longer than it likes.
    pub(super) fn hold_shutdown(&self, why: &str) -> Box<dyn Send> {
        *self.shared.holds.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        if let Some(window) = self.window {
            let reason = Box::into_raw(Box::new(wide(why)));
            // SAFETY: a window of this process; the reason is handed over to
            // its procedure, which frees it.
            let posted = unsafe { PostMessageW(window as HWND, HOLD, 0, reason as LPARAM) };
            if posted == 0 {
                // SAFETY: not posted, so still this function's to free.
                drop(unsafe { Box::from_raw(reason) });
            }
        }
        Box::new(Hold {
            shared: Arc::clone(&self.shared),
            window: self.window,
        })
    }
}

struct Hold {
    shared: Arc<Shared>,
    window: Option<isize>,
}

impl Drop for Hold {
    fn drop(&mut self) {
        {
            let mut holds = self.shared.holds.lock().unwrap_or_else(|e| e.into_inner());
            *holds = holds.saturating_sub(1);
        }
        self.shared.released.notify_all();
        if let Some(window) = self.window {
            // SAFETY: a window of this process, sent a message with no
            // pointer in it.
            unsafe { PostMessageW(window as HWND, RELEASE, 0, 0) };
        }
    }
}

/// What Windows calls when the network changes.
unsafe extern "system" fn network_changed(
    context: *const core::ffi::c_void,
    hint: NL_NETWORK_CONNECTIVITY_HINT,
) {
    let shared = context.cast::<Shared>();
    if !shared.is_null() {
        // SAFETY: the pointer `listen` registered: an `Arc`'s, kept alive
        // for the life of the process by the reference leaked there.
        unsafe { &*shared }.network_is(hint.ConnectivityLevel);
    }
}

/// The window, on a thread of its own; `None` when none could be made, which
/// leaves the settings as they were read at the start.
fn window(shared: Arc<Shared>) -> Option<isize> {
    let (made, window) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("system".into())
        .spawn(move || {
            let made_one = make_window(shared);
            let _ = made.send(made_one);
            if made_one.is_some() {
                pump();
            }
        });
    let found = spawned.ok().and_then(|_| window.recv().ok().flatten());
    if found.is_none() {
        tracing::debug!("no window to hear Windows in");
    }
    found
}

/// Registers the class, once for the process.
fn class(instance: windows_sys::Win32::Foundation::HINSTANCE) -> bool {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    *REGISTERED.get_or_init(|| {
        let class = wide(CLASS);
        let registered = WNDCLASSW {
            lpfnWndProc: Some(answer),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..WNDCLASSW::default()
        };
        // SAFETY: the structure is complete, and its class name,
        // NUL-terminated, outlives the call, which copies it.
        unsafe { RegisterClassW(&raw const registered) != 0 }
    })
}

/// Makes the window on this thread, with `shared` where its procedure finds
/// it, for the life of the process.
fn make_window(shared: Arc<Shared>) -> Option<isize> {
    // SAFETY: a null name asks for this executable's own module handle.
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    if !class(instance) {
        return None;
    }
    let class = wide(CLASS);
    // SAFETY: the class is registered; every other argument is a plain value
    // or null, and the window is never shown.
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if window.is_null() {
        return None;
    }
    let kept = Arc::into_raw(shared);
    // SAFETY: `window` was made above, and the value stored is a pointer the
    // procedure reads back as what it is.
    unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, kept as isize) };
    Some(window as isize)
}

/// The window's thread: messages, for as long as the process lives.
fn pump() {
    let mut message = MSG::default();
    // SAFETY: `message` is valid for the call to write; a null window reads
    // every message of this thread, whose only window is the one above.
    while unsafe { GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) } > 0 {
        // SAFETY: a message `GetMessageW` just filled.
        unsafe { DispatchMessageW(&raw const message) };
    }
}

/// What the window says to what it is sent.
unsafe extern "system" fn answer(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: reads back what `make_window` stored on this window.
    let kept = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Shared;
    // SAFETY: when set, the pointer is an `Arc`'s, kept alive for the life
    // of the process by the reference the window holds.
    let shared = unsafe { kept.as_ref() };
    match (message, shared) {
        (WM_SETTINGCHANGE | WM_THEMECHANGED, Some(shared)) => {
            shared.settings_may_have_changed();
            0
        }
        (WM_POWERBROADCAST, Some(shared)) => {
            // The battery saver came on or went off, among other things.
            if wparam == PBT_APMPOWERSTATUSCHANGE as WPARAM {
                shared.settings_may_have_changed();
            }
            1
        }
        (HOLD, _) => {
            // SAFETY: `hold_shutdown` boxed this string and handed it over
            // with this message, which nothing else sends.
            let reason = unsafe { Box::from_raw(lparam as *mut Vec<u16>) };
            // SAFETY: this thread's own window, and a NUL-terminated string
            // the call copies.
            unsafe { ShutdownBlockReasonCreate(window, reason.as_ptr()) };
            0
        }
        (RELEASE, Some(shared)) => {
            if *shared.holds.lock().unwrap_or_else(|e| e.into_inner()) == 0 {
                // SAFETY: this thread's own window.
                unsafe { ShutdownBlockReasonDestroy(window) };
            }
            0
        }
        // Yes, the session may end: what is being saved is waited for when
        // it does, which Windows grants without asking the person.
        (WM_QUERYENDSESSION, _) => 1,
        (WM_ENDSESSION, Some(shared)) => {
            if wparam != 0 {
                shared.say(Event::ShuttingDown);
                let holds = shared.holds.lock().unwrap_or_else(|e| e.into_inner());
                let _ = shared
                    .released
                    .wait_timeout_while(holds, PATIENCE, |holds| *holds > 0);
            }
            0
        }
        // SAFETY: every other message is the default procedure's, with the
        // arguments it was sent.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Networking::WinSock::{
        NetworkConnectivityLevelHintHidden, NetworkConnectivityLevelHintLocalAccess,
        NetworkConnectivityLevelHintUnknown,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    use tokio::sync::mpsc;

    use super::*;

    /// A listener of the test's own, and what it says.
    fn listening() -> (Windows, mpsc::UnboundedReceiver<Event>) {
        let (said, heard) = mpsc::unbounded_channel();
        let windows = Windows::listen(Arc::new(Mutex::new(Settings::default())), move |event| {
            let _ = said.send(event);
        });
        (windows, heard)
    }

    fn send(window: isize, message: u32, wparam: WPARAM) -> LRESULT {
        // SAFETY: a window of this process, sent a message it answers.
        unsafe { SendMessageW(window as HWND, message, wparam, 0) }
    }

    #[test]
    fn what_windows_is_set_to_reads_as_something_the_app_can_follow() {
        let mut settings = Settings::default();
        read_into(&mut settings);
        assert!(
            (1.0..=2.25).contains(&settings.text_scale),
            "{}",
            settings.text_scale
        );
    }

    #[test]
    fn only_no_network_at_all_is_offline() {
        assert_eq!(network(NetworkConnectivityLevelHintNone), OFFLINE);
        assert_eq!(network(NetworkConnectivityLevelHintInternetAccess), ONLINE);
        assert_eq!(
            network(NetworkConnectivityLevelHintConstrainedInternetAccess),
            ONLINE
        );
        for unsure in [
            NetworkConnectivityLevelHintUnknown,
            NetworkConnectivityLevelHintLocalAccess,
            NetworkConnectivityLevelHintHidden,
        ] {
            assert_eq!(network(unsure), NOT_SAID);
        }
    }

    #[test]
    fn the_network_is_said_when_it_goes_and_when_it_is_back_and_not_before() {
        let (windows, mut heard) = listening();
        windows.shared.network.store(NOT_SAID, Ordering::Relaxed);

        windows
            .shared
            .network_is(NetworkConnectivityLevelHintInternetAccess);
        assert!(heard.try_recv().is_err(), "there being one is no news");
        assert_eq!(windows.online(), Some(true));

        windows.shared.network_is(NetworkConnectivityLevelHintNone);
        assert_eq!(heard.try_recv().ok(), Some(Event::Online(false)));
        assert_eq!(windows.online(), Some(false));
        windows.shared.network_is(NetworkConnectivityLevelHintNone);
        assert!(heard.try_recv().is_err(), "said once");

        windows
            .shared
            .network_is(NetworkConnectivityLevelHintLocalAccess);
        assert_eq!(heard.try_recv().ok(), Some(Event::Online(true)));
        assert_eq!(windows.online(), None);
    }

    #[test]
    fn a_change_of_settings_is_said_once_and_only_when_one_changed() {
        let (windows, mut heard) = listening();
        let window = windows.window.expect("a window");
        send(window, WM_SETTINGCHANGE, 0);
        assert!(heard.try_recv().is_err(), "nothing changed");

        // As if the text had been at another size until now.
        windows.shared.settings.lock().unwrap().text_scale = 9.0;
        send(window, WM_SETTINGCHANGE, 0);
        let Some(Event::Settings(settings)) = heard.try_recv().ok() else {
            panic!("the change was not said");
        };
        assert!((1.0..=2.25).contains(&settings.text_scale));
        send(window, WM_THEMECHANGED, 0);
        assert!(heard.try_recv().is_err(), "said once");
    }

    #[test]
    fn the_end_of_the_session_is_said_and_waits_for_what_holds_it() {
        let (windows, mut heard) = listening();
        let window = windows.window.expect("a window");
        assert_eq!(
            send(window, WM_QUERYENDSESSION, 0),
            1,
            "the session may end"
        );

        let hold = windows.hold_shutdown("Saving");
        let ending = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            send(window, WM_ENDSESSION, 1);
            started.elapsed()
        });
        let said = loop {
            match heard.try_recv() {
                Ok(event) => break event,
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        assert_eq!(said, Event::ShuttingDown);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!ending.is_finished(), "held while something is being saved");
        drop(hold);
        let held = ending.join().unwrap();
        assert!(held < PATIENCE, "only until it was saved: {held:?}");

        // With nothing held, the end is not kept waiting at all.
        let started = std::time::Instant::now();
        send(window, WM_ENDSESSION, 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
