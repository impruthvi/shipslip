//! Remote execution boundary. The engine only talks to servers through
//! [`Transport`], so tests can use a fake and other platforms can add their
//! own backend.

use std::future::Future;

use tokio::sync::mpsc::Sender;

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
    /// Output lines are sent to `output` as they arrive; a full channel
    /// pauses reading, so the caller must drain it while the future runs.
    /// Lines longer than `line_limit` bytes are cut, ending in
    /// `… [line truncated, N bytes]`. `output` must be dropped by the time
    /// the future resolves. `Ok(code)` is the remote exit status (any value,
    /// including 255). Deploy steps do not rely on the script surviving a
    /// dropped future: they run detached on the server.
    fn run(
        &self,
        script: &str,
        output: Sender<String>,
        line_limit: usize,
    ) -> impl Future<Output = Result<i32, TransportError>> + Send;

    /// Replaces a lost connection with a new one. Runs nothing remotely.
    fn reconnect(&self) -> impl Future<Output = Result<(), TransportError>> + Send;
}

/// Lines queued between a transport and its reader.
pub const LINE_QUEUE: usize = 256;

/// Longest line kept from a deploy step's own output.
pub const STEP_LINE_LIMIT: usize = 16 * 1024;

/// Longest line kept from Shipslip's own scripts, whose log data arrives as
/// one base64 line already limited on the server.
pub const SCRIPT_LINE_LIMIT: usize = 64 * 1024 * 1024;

/// The text that replaces the end of a line longer than the limit.
pub fn truncation_marker(dropped: usize) -> String {
    format!("… [line truncated, {dropped} bytes]")
}

#[cfg(unix)]
mod ssh;
#[cfg(unix)]
pub use ssh::SshTransport;
