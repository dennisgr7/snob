//! Where the owner listens and how a command reaches it: one
//! [`local_socket`](crate::local_socket) per data directory, which admits
//! only the user running snob, on both ends.

use std::io;

use snob_store::paths::AppPaths;

use crate::local_socket::{self, Endpoint};
pub(super) use crate::local_socket::{
    Listener, Owner, ServerStream, Stream, nobody_there, owner_of,
};

/// The owner's place. **The names it had before it was one of several.**
const OWNER: Endpoint = Endpoint {
    file: "browser-owner",
    pipe: "browsers",
};

/// Listens, or `None` when another owner already does.
pub(super) fn bind(paths: &AppPaths) -> io::Result<Option<Listener>> {
    local_socket::bind(paths, &OWNER)
}

/// Reaches the owner, if one is listening and it is this user's.
pub(super) async fn connect(paths: &AppPaths) -> io::Result<Stream> {
    local_socket::connect(paths, &OWNER).await
}
