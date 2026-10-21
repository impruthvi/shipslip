//! [`Transport`] over the system OpenSSH client: one ControlMaster per
//! server, each script run as a native-mux session.

use std::error::Error as StdError;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use openssh::{Error, KnownHosts, Session, SessionBuilder, Stdio};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::Sender;

use super::{truncation_marker, Transport, TransportError};

/// A connection to one server. Dropping it closes the master connection.
#[derive(Debug)]
pub struct SshTransport {
    alias: String,
    config_file: Option<PathBuf>,
    session: Mutex<Arc<Session>>,
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
        let session = connect_session(alias, config_file).await?;
        Ok(Self {
            alias: alias.to_string(),
            config_file: config_file.map(Path::to_path_buf),
            session: Mutex::new(Arc::new(session)),
        })
    }
}

async fn connect_session(
    alias: &str,
    config_file: Option<&Path>,
) -> Result<Session, TransportError> {
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
    builder.connect_mux(alias).await.map_err(connect_error)
}

impl Transport for SshTransport {
    async fn run(
        &self,
        script: &str,
        output: Sender<String>,
        line_limit: usize,
    ) -> Result<i32, TransportError> {
        let session = self.session.lock().unwrap().clone();
        let mut child = session
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
            forward_lines(child.stdout().take(), &output, line_limit),
            forward_lines(child.stderr().take(), &output, line_limit),
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

    async fn reconnect(&self) -> Result<(), TransportError> {
        let session = connect_session(&self.alias, self.config_file.as_deref()).await?;
        *self.session.lock().unwrap() = Arc::new(session);
        Ok(())
    }
}

async fn forward_lines(
    stream: Option<impl AsyncRead + Unpin>,
    output: &Sender<String>,
    line_limit: usize,
) -> io::Result<()> {
    let Some(stream) = stream else {
        return Ok(());
    };
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    while let Some(dropped) = read_line_capped(&mut reader, &mut buf, line_limit).await? {
        let mut line = String::from_utf8_lossy(&buf).into_owned();
        if dropped > 0 {
            line.push_str(&truncation_marker(dropped));
        }
        // A closed channel means nobody reads the output; keep draining so
        // the remote command is not blocked on a full pipe.
        let _ = output.send(line).await;
    }
    Ok(())
}

/// Reads one line into `buf` without its line ending, keeping at most
/// `limit` bytes. Returns the number of bytes cut, or `None` at the end.
async fn read_line_capped(
    reader: &mut (impl AsyncBufRead + Unpin),
    buf: &mut Vec<u8>,
    limit: usize,
) -> io::Result<Option<usize>> {
    buf.clear();
    let mut dropped = 0;
    let mut read_any = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(read_any.then_some(dropped));
        }
        read_any = true;
        let newline = available.iter().position(|&byte| byte == b'\n');
        let chunk = &available[..newline.unwrap_or(available.len())];
        let keep = chunk.len().min(limit.saturating_sub(buf.len()));
        buf.extend_from_slice(&chunk[..keep]);
        dropped += chunk.len() - keep;
        let consumed = chunk.len() + usize::from(newline.is_some());
        reader.consume(consumed);
        if newline.is_some() {
            if dropped == 0 && buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(Some(dropped));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn lines(input: &[u8], limit: usize) -> Vec<(String, usize)> {
        let mut reader = BufReader::with_capacity(7, input);
        let mut buf = Vec::new();
        let mut lines = Vec::new();
        while let Some(dropped) = read_line_capped(&mut reader, &mut buf, limit)
            .await
            .unwrap()
        {
            lines.push((String::from_utf8(buf.clone()).unwrap(), dropped));
        }
        lines
    }

    #[tokio::test]
    async fn lines_are_split_without_endings_and_cut_at_the_limit() {
        assert_eq!(
            lines(b"one\r\ntwo\n\nlast", 10).await,
            [
                ("one".into(), 0),
                ("two".into(), 0),
                (String::new(), 0),
                ("last".into(), 0)
            ]
        );
        assert_eq!(
            lines(b"0123456789abcdef\nnext\n", 10).await,
            [("0123456789".into(), 6), ("next".into(), 0)]
        );
        assert!(lines(b"", 10).await.is_empty());
    }

    #[tokio::test]
    async fn a_huge_line_without_a_newline_keeps_only_the_limit() {
        let input = vec![b'x'; 10 * 1024 * 1024];
        let read = lines(&input, 16 * 1024).await;
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].0.len(), 16 * 1024);
        assert_eq!(read[0].1, 10 * 1024 * 1024 - 16 * 1024);
    }
}
