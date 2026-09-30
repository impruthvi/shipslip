//! Remote execution boundary. The engine only talks to servers through
//! [`Transport`], so tests can use a fake and other platforms can add their
//! own backend.

use std::future::Future;

use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// The connection dropped while a command was running; its result is unknown.
    #[error("connection lost: {0}")]
    ConnectionLost(String),
    /// The script was never started, so nothing on the server changed.
    #[error("could not connect: {0}")]
    Connect(String),
}

/// Remote execution on one server.
pub trait Transport: Send + Sync + 'static {
    /// Runs a bash script on the remote host.
    ///
    /// Output lines are sent to `output` as they arrive; `output` must be
    /// dropped by the time the future resolves. `Ok(code)` is the remote exit
    /// status (any value, including 255). Deploy steps do not rely on the
    /// script surviving a dropped future: they run detached on the server.
    fn run(
        &self,
        script: &str,
        output: UnboundedSender<String>,
    ) -> impl Future<Output = Result<i32, TransportError>> + Send;

    /// Replaces a lost connection with a new one. Runs nothing remotely.
    fn reconnect(&self) -> impl Future<Output = Result<(), TransportError>> + Send;
}

#[cfg(unix)]
mod ssh;
#[cfg(unix)]
pub use ssh::SshTransport;
