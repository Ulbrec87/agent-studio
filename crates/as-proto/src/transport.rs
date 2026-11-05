//! Local-socket helpers: a Unix domain socket under the data root, a named pipe on Windows.
//! The name comes from [`Paths::daemon_socket_name`] (a plain path on both platforms).

use std::io;

use as_core::paths::Paths;
use interprocess::local_socket::tokio::{Listener, Stream};
use interprocess::local_socket::traits::tokio::Listener as _;
#[cfg(not(windows))]
use interprocess::local_socket::traits::tokio::Stream as _;
use interprocess::local_socket::{GenericFilePath, ListenerOptions, ToFsName};

/// Client side of a connection.
///
/// Windows: tokio's own named-pipe client. interprocess' tokio connect waits on a blocking thread
/// with NO timeout while the pipe is busy (no listening instance, e.g. while the last server-side
/// instances of a stopped daemon still linger) and ignores `ConnectWaitMode`; that thread outlives
/// every `tokio::time::timeout` around it and blocks the runtime's shutdown forever.
#[cfg(windows)]
pub type Conn = tokio::net::windows::named_pipe::NamedPipeClient;
#[cfg(not(windows))]
pub type Conn = Stream;
/// Server side of a connection, as returned by [`accept`].
pub type Accepted = Stream;
pub type Server = Listener;

fn name(socket: &str) -> io::Result<interprocess::local_socket::Name<'static>> {
    socket.to_owned().to_fs_name::<GenericFilePath>()
}

/// Connect to the socket named `socket`.
pub async fn connect_name(socket: &str) -> io::Result<Conn> {
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        /// ERROR_PIPE_BUSY: every instance is connected right now; retry shortly.
        const ERROR_PIPE_BUSY: i32 = 231;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match ClientOptions::new().open(socket) {
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                        && std::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                other => return other,
            }
        }
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

pub async fn accept(server: &Server) -> io::Result<Accepted> {
    server.accept().await
}
