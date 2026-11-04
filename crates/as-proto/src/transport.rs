//! Local-socket helpers: a Unix domain socket under the data root, a named pipe on Windows.
//! The name comes from [`Paths::daemon_socket_name`] (a plain path on both platforms).

use std::io;

use as_core::paths::Paths;
use interprocess::local_socket::tokio::{Listener, Stream};
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::{GenericFilePath, ListenerOptions, ToFsName};

pub type Conn = Stream;
pub type Server = Listener;

fn name(socket: &str) -> io::Result<interprocess::local_socket::Name<'static>> {
    socket.to_owned().to_fs_name::<GenericFilePath>()
}

/// Connect to the socket named `socket`.
pub async fn connect_name(socket: &str) -> io::Result<Conn> {
    Stream::connect(name(socket)?).await
}

pub async fn connect(paths: &Paths) -> io::Result<Conn> {
    connect_name(&paths.daemon_socket_name()).await
}

/// Bind the daemon socket. The caller has checked that no live daemon owns it; a leftover
/// socket file from a crashed daemon is removed first (Unix).
pub fn bind(paths: &Paths) -> io::Result<Server> {
    remove_socket_file(paths);
    ListenerOptions::new()
        .name(name(&paths.daemon_socket_name())?)
        .create_tokio()
}

/// Delete the socket file (no-op for Windows named pipes).
pub fn remove_socket_file(paths: &Paths) {
    if !cfg!(windows) {
        let _ = std::fs::remove_file(paths.daemon_socket_name());
    }
}

pub async fn accept(server: &Server) -> io::Result<Conn> {
    server.accept().await
}
