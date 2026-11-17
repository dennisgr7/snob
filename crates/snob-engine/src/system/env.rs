//! Switches read from the environment.

/// Whether a switch in the environment is on: set, and neither empty nor `0`.
pub fn flag(name: &str) -> bool {
    is_on(std::env::var_os(name).as_deref())
}

fn is_on(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

#[cfg(test)]
mod tests {
    use super::is_on;

    /// `=0` and an empty value leave a switch off, as unset does.
    #[test]
    fn a_switch_is_on_only_when_it_says_so() {
        let on = |value: Option<&str>| is_on(value.map(std::ffi::OsStr::new));
        assert!(on(Some("1")));
        assert!(on(Some("yes")));
        assert!(!on(Some("0")));
        assert!(!on(Some("")));
        assert!(!on(None));
    }
}
