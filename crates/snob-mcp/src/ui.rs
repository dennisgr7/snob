//! The lines the server says outside the protocol: on standard error, as the
//! engine's notices, and a line of a result on standard output.

pub(crate) use snob_engine::host::{info, warn};

/// `println!` that survives the reader leaving (`files::say_line`).
macro_rules! say {
    ($($arg:tt)*) => {
        snob_engine::files::say_line(format_args!($($arg)*))
    };
}
pub(crate) use say;
