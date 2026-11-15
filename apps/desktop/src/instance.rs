//! One instance: a second start of the app hands over to the first, which
//! comes to the front, and leaves. **The command line is no instance of the
//! app**: both run at once, and the engine's own locks keep them apart.
//!
//! The first instance listens where only this user reaches it
//! (`snob_engine::local_socket`), one listener per data directory, so a test
//! build in its sandbox is never taken for the real app.

use std::time::Duration;

use snob_engine::local_socket::{self, Endpoint, Listener};
use snob_store::paths::AppPaths;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// Where the first instance listens.
const INSTANCE: Endpoint = Endpoint {
    file: "desktop",
    pipe: "desktop",
};

/// What a second start says to the first.
const ACTIVATE: &[u8] = b"activate\n";

/// How long either end waits for the other.
const PATIENCE: Duration = Duration::from_secs(2);

/// Which start this is.
pub enum Instance {
    /// The first: it listens for those that come after.
    First(Listener),
    /// Another already runs, and was told to come to the front.
    Second,
}

/// Claims the place of the first instance, or hands over to whoever has it.
///
/// **A listener that does not answer is not waited on forever**: a first
/// instance that hangs must not keep the app from ever opening again, so the
/// second start gives up on it and still leaves, having changed nothing.
pub async fn claim(paths: &AppPaths) -> std::io::Result<Instance> {
    paths.ensure_dirs().map_err(std::io::Error::other)?;
    if let Some(listener) = local_socket::bind(paths, &INSTANCE)? {
        return Ok(Instance::First(listener));
    }
    let handed_over = async {
        let mut first = local_socket::connect(paths, &INSTANCE).await?;
        first.write_all(ACTIVATE).await?;
        first.flush().await
    };
    match tokio::time::timeout(PATIENCE, handed_over).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "the running app could not be told to open"),
        Err(_) => tracing::warn!("the running app did not answer"),
    }
    Ok(Instance::Second)
}

/// Listens for later starts, calling `activate` for each, until dropped.
pub async fn serve(mut listener: Listener, activate: impl Fn() + Send + 'static) {
    loop {
        let mut stream = match listener.accept().await {
            Ok(stream) => stream,
            Err(e) => {
                tracing::warn!(error = %e, "the app stopped listening for a second start");
                return;
            }
        };
        let mut said = [0u8; ACTIVATE.len()];
        let heard = tokio::time::timeout(PATIENCE, stream.read_exact(&mut said)).await;
        if matches!(heard, Ok(Ok(_))) && said == ACTIVATE {
            tracing::info!("a second start handed over");
            activate();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn a_second_start_hands_over_to_the_first_and_leaves() {
        let root = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(root.path());

        let Instance::First(listener) = claim(&paths).await.unwrap() else {
            panic!("nothing else runs in a new directory");
        };
        let activated = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&activated);
        let serving = tokio::spawn(serve(listener, move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }));

        for _ in 0..2 {
            assert!(matches!(claim(&paths).await.unwrap(), Instance::Second));
        }
        for _ in 0..100 {
            if activated.load(Ordering::SeqCst) == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(activated.load(Ordering::SeqCst), 2);
        serving.abort();
    }

    #[tokio::test]
    async fn another_data_directory_is_another_app() {
        let (one, other) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let first = claim(&AppPaths::rooted_at(one.path())).await.unwrap();
        let elsewhere = claim(&AppPaths::rooted_at(other.path())).await.unwrap();
        assert!(matches!(first, Instance::First(_)));
        assert!(matches!(elsewhere, Instance::First(_)));
    }
}
