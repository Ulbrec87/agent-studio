//! Local-socket helpers: a Unix domain socket under the data root, a named pipe on Windows.
//! The name comes from [`Paths::daemon_socket_name`] (a plain path on both platforms).

use std::io;

use as_core::paths::Paths;
use interprocess::local_socket::tokio::{Listener, Stream};
use interprocess::local_socket::traits::tokio::Listener as _;
#[cfg(not(windows))]
use interprocess::local_socket::traits::tokio::Stream as _;
use interprocess::local_socket::{GenericFilePath, ListenerOptions, ToFsName};

pub type Conn = Stream;
pub type Server = Listener;

fn name(socket: &str) -> io::Result<interprocess::local_socket::Name<'static>> {
    socket.to_owned().to_fs_name::<GenericFilePath>()
}

/// Connect to the socket named `socket`.
pub async fn connect_name(socket: &str) -> io::Result<Conn> {
    #[cfg(windows)]
    {
        // A busy named pipe (no listening instance) makes interprocess wait on a blocking thread
        // with NO timeout by default; that thread outlives our `tokio::time::timeout` callers and
        // blocks runtime shutdown forever. Bound it.
        use interprocess::ConnectWaitMode;
        use interprocess::local_socket::ConnectOptions;
        let options = ConnectOptions::new()
            .name(name(socket)?)
            .wait_mode(ConnectWaitMode::Timeout(std::time::Duration::from_secs(2)));
        options.connect_tokio().await
    }
    #[cfg(not(windows))]
    {
        Stream::connect(name(socket)?).await
    }
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
