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

impl TransportError {
    /// Classifies a connection refusal using OpenSSH's diagnostic text.
    /// A lost connection has an unknown command result, not a refusal.
    pub fn connect_failure(&self) -> Option<ConnectFailure> {
        match self {
            Self::Connect(message) => Some(ConnectFailure::of(message)),
            Self::ConnectionLost(_) => None,
        }
    }
}

/// Why OpenSSH refused a connection. Unrecognized diagnostics stay [`Self::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectFailure {
    /// No trusted host key was found. Compare the fingerprint in Terminal.
    HostKeyUnknown,
    /// The trusted host key changed. Never offer a command to trust its replacement.
    HostKeyChanged,
    /// None of the offered keys was accepted, possibly because the key is not loaded.
    KeyNotLoaded,
    /// No specific refusal was recognized; show the original diagnostic.
    Other,
}

impl ConnectFailure {
    /// Classifies diagnostic text; the changed-key banner takes precedence
    /// because it also includes the generic host-key verification failure.
    pub fn of(message: &str) -> Self {
        if message.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
            Self::HostKeyChanged
        } else if message.contains("Host key verification failed") {
            Self::HostKeyUnknown
        } else if message.contains("Permission denied (publickey") {
            Self::KeyNotLoaded
        } else {
            Self::Other
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_refusals_preserve_the_original_diagnostic() {
        for (message, expected) in [
            (
                "Host key verification failed.",
                ConnectFailure::HostKeyUnknown,
            ),
            (
                "REMOTE HOST IDENTIFICATION HAS CHANGED!\nHost key verification failed.",
                ConnectFailure::HostKeyChanged,
            ),
            (
                "Permission denied (publickey).",
                ConnectFailure::KeyNotLoaded,
            ),
            (
                "Permission denied (publickey,password).",
                ConnectFailure::KeyNotLoaded,
            ),
            ("Connection refused", ConnectFailure::Other),
            ("Could not resolve hostname server", ConnectFailure::Other),
            ("unrecognized diagnostic", ConnectFailure::Other),
            ("", ConnectFailure::Other),
        ] {
            let error = TransportError::Connect(message.into());
            assert_eq!(error.connect_failure(), Some(expected), "{message}");
            assert_eq!(error.to_string(), format!("could not connect: {message}"));
        }
    }

    #[test]
    fn a_lost_connection_is_not_a_refusal() {
        let error = TransportError::ConnectionLost("Host key verification failed".into());
        assert_eq!(error.connect_failure(), None);
    }
}
