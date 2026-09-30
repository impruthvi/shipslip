//! [`Transport`] over the system OpenSSH client: one ControlMaster per
//! server, each script run as a native-mux session.

use std::error::Error as StdError;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use openssh::{Error, KnownHosts, Session, SessionBuilder, Stdio};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::UnboundedSender;

use super::{Transport, TransportError};

/// A connection to one server. Dropping it closes the master connection.
///
/// Known gap: a script is not guaranteed to keep running once its `run`
/// future is dropped (detach), until steps run detached on the server.
#[derive(Debug)]
pub struct SshTransport {
    session: Session,
}

impl SshTransport {
    /// Connects to `alias` as resolved by the user's ssh config.
    pub async fn connect(alias: &str) -> Result<Self, TransportError> {
        Self::open(alias, None).await
    }

    /// Connects using only `config_file` instead of the user's ssh config.
    pub async fn connect_with_config(
        alias: &str,
        config_file: &Path,
    ) -> Result<Self, TransportError> {
        Self::open(alias, Some(config_file)).await
    }

    async fn open(alias: &str, config_file: Option<&Path>) -> Result<Self, TransportError> {
        if alias.is_empty() || alias.starts_with('-') {
            return Err(TransportError::Connect(format!(
                "invalid ssh alias `{alias}`"
            )));
        }
        let mut builder = SessionBuilder::default();
        builder
            .known_hosts_check(KnownHosts::Strict)
            .connect_timeout(Duration::from_secs(15))
            .server_alive_interval(Duration::from_secs(15))
            .control_directory(control_dir().map_err(connect_error)?);
        if let Some(path) = config_file {
            builder.config_file(path);
        }
        let session = builder.connect_mux(alias).await.map_err(connect_error)?;
        Ok(Self { session })
    }
}

impl Transport for SshTransport {
    async fn run(
        &self,
        script: &str,
        output: UnboundedSender<String>,
    ) -> Result<i32, TransportError> {
        let mut child = self
            .session
            .command("bash")
            .args(["-l", "-s"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .await
            .map_err(connect_error)?;

        let stdin = child.stdin().take();
        let send = async {
            // A write error means bash stopped reading (e.g. `cd` failed and it
            // exited). The step body sits inside `( ... )`, so a truncated
            // script is a syntax error, never a partial step; `wait` decides.
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(script.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }
        };
        let (_, out, err) = tokio::join!(
            send,
            forward_lines(child.stdout().take(), &output),
            forward_lines(child.stderr().take(), &output),
        );
        drop(output);
        out.map_err(lost)?;
        err.map_err(lost)?;

        match child.wait().await {
            Ok(status) => status
                .code()
                .ok_or_else(|| TransportError::ConnectionLost("no exit status".into())),
            // native-mux reports exit 127 as "command not found". The remote
            // command is bash itself, so the 127 came from the script.
            Err(Error::Remote(e)) if e.kind() == io::ErrorKind::NotFound => Ok(127),
            // Also covers a script killed by a signal: the mux protocol cannot
            // tell that apart from a dropped connection.
            Err(e) => Err(lost(e)),
        }
    }
}

async fn forward_lines(
    stream: Option<impl AsyncRead + Unpin>,
    output: &UnboundedSender<String>,
) -> io::Result<()> {
    let Some(stream) = stream else {
        return Ok(());
    };
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf).await? == 0 {
            return Ok(());
        }
        let line = buf.strip_suffix(b"\n").unwrap_or(&buf);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let _ = output.send(String::from_utf8_lossy(line).into_owned());
    }
}

/// `~/.ssh`, created if missing. The control socket lives here because
/// macOS limits socket paths to 104 bytes, which temp dirs can exceed.
fn control_dir() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let dir = PathBuf::from(home).join(".ssh");
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(dir),
    }
}

fn describe(e: &dyn StdError) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(inner) = source {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        source = inner.source();
    }
    text
}

fn connect_error(e: impl StdError) -> TransportError {
    TransportError::Connect(describe(&e))
}

fn lost(e: impl StdError) -> TransportError {
    TransportError::ConnectionLost(describe(&e))
}
