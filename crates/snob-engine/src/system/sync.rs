//! A lock taken whatever happened to whoever held it last.

/// What `mutex` guards, whether or not a thread panicked holding it: every
/// value kept behind one here stays whole whatever panicked.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
