//! Runs deploy steps detached on the server, so a step survives a dropped
//! connection, a sleeping laptop, or a detach, and can be re-attached.
//!
//! Each step gets `~/.shipslip/runs/<run_id>/<key>/` holding `script.sh`,
//! `log` (stdout + stderr), `pid`, and `exit` (written atomically when the
//! step ends). A step is launched at most once: nothing here ever re-sends
//! the launch.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::script::shell_quote;
use crate::transport::{Transport, TransportError};

/// Waits before each reconnect attempt after the connection drops.
const RECONNECT_DELAYS: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];

/// Upper bound on connection drops survived by a single step.
const MAX_REATTACHES: usize = 5;

/// Printed by the observer before the log, so anything a login shell prints
/// first is not taken for step output.
pub(crate) const LOG_START: &str = "shipslip-log-start";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StepResult {
    Exited(i32),
    /// The step was never started on the server.
    NotStarted(String),
    /// The step's process ended without recording an exit code.
    Gone,
    /// The server could not be reached again; the step may still be running.
    Interrupted(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Probe {
    Exited(i32),
    Running,
    Gone,
    NotStarted,
}

fn run_dir(run_id: &str) -> String {
    format!("\"$HOME\"/.shipslip/runs/{}", shell_quote(run_id))
}

pub(crate) fn launch_script(run_id: &str, key: &str, script: &str) -> String {
    format!(
        r#"set -e
umask 077
mkdir -p {runs}
chmod 700 "$HOME"/.shipslip "$HOME"/.shipslip/runs
printf '%s' {script} > {runs}/{key}.sh
mkdir {runs}/{key}
d={runs}/{key}
mv "$d.sh" "$d/script.sh"
setsid nohup bash -c 'echo $$ > "$1/pid.tmp" && mv "$1/pid.tmp" "$1/pid"; bash -l -s < "$1/script.sh"; echo $? > "$1/exit.tmp"; mv "$1/exit.tmp" "$1/exit"' _ "$d" > "$d/log" 2>&1 < /dev/null &
for _ in $(seq 100); do [ -s "$d/pid" ] && exit 0; sleep 0.05; done
exit 1
"#,
        runs = run_dir(run_id),
        script = shell_quote(script),
    )
}

/// Streams the step's log from line `skip + 1` until the step's process ends.
pub(crate) fn observe_script(run_id: &str, key: &str, skip: usize) -> String {
    format!(
        "d={runs}/{key}\n\
         echo {LOG_START}\n\
         exec tail -n +{from} -s 0.2 --pid=\"$(cat \"$d/pid\")\" -f \"$d/log\" 2>/dev/null\n",
        runs = run_dir(run_id),
        from = skip + 1,
    )
}

pub(crate) fn probe_script(run_id: &str, key: &str) -> String {
    format!(
        r#"d={runs}/{key}
state() {{
  if [ -e "$d/exit" ]; then echo "exited $(cat "$d/exit")"
  elif [ ! -d "$d" ]; then echo not-started
  elif [ -s "$d/pid" ] && kill -0 "$(cat "$d/pid")" 2>/dev/null; then echo running
  else return 1
  fi
}}
state || {{ sleep 1; state || echo gone; }}
"#,
        runs = run_dir(run_id),
    )
}

fn parse_probe(lines: &[String]) -> Option<Probe> {
    // Login shell startup files may print first; the answer is the last line.
    let last = lines.iter().rev().find(|l| !l.trim().is_empty())?.trim();
    match last {
        "running" => Some(Probe::Running),
        "gone" => Some(Probe::Gone),
        "not-started" => Some(Probe::NotStarted),
        _ => last
            .strip_prefix("exited ")?
            .parse()
            .ok()
            .map(Probe::Exited),
    }
}

/// Launches `script` detached as `key` (e.g. `step-2`) of `run_id` and
/// follows it to the end, re-attaching after dropped connections. Output
/// lines go to `output`.
pub(crate) async fn run_step<T: Transport>(
    transport: &T,
    run_id: &str,
    key: &str,
    script: &str,
    output: &mpsc::UnboundedSender<String>,
) -> StepResult {
    let (launch, launch_output) = run_collect(transport, &launch_script(run_id, key, script)).await;
    if let Err(e @ TransportError::Connect(_)) = &launch {
        return StepResult::NotStarted(e.to_string());
    }
    let mut reattaches = 0;
    if launch != Ok(0) {
        // Unclear whether the step started: ask the server. Never launch again.
        let mut last = launch.clone();
        loop {
            if let Err(reason) = reconnect_if_lost(transport, &last, &mut reattaches).await {
                return StepResult::Interrupted(reason);
            }
            let (result, lines) = run_collect(transport, &probe_script(run_id, key)).await;
            if result.is_err() {
                last = result;
                continue;
            }
            match parse_probe(&lines) {
                Some(Probe::NotStarted) => {
                    return StepResult::NotStarted(match &launch {
                        Err(e) => e.to_string(),
                        Ok(code) => {
                            format!("launcher exited with {code}: {}", launch_output.join("\n"))
                        }
                    })
                }
                Some(_) => break,
                None => return unexpected(&lines),
            }
        }
    }

    follow_existing(transport, run_id, key, output, &mut reattaches).await
}

/// Observes a previously launched step without sending its command again.
pub(crate) async fn attach_step<T: Transport>(
    transport: &T,
    run_id: &str,
    key: &str,
    output: &mpsc::UnboundedSender<String>,
) -> StepResult {
    let mut reattaches = 0;
    loop {
        let (result, lines) = run_collect(transport, &probe_script(run_id, key)).await;
        if let Err(reason) = reconnect_if_lost(transport, &result, &mut reattaches).await {
            return StepResult::Interrupted(reason);
        }
        if result.is_err() {
            continue;
        }
        match parse_probe(&lines) {
            Some(Probe::NotStarted) => {
                return StepResult::NotStarted(
                    "the step was not launched before Shipslip exited".into(),
                )
            }
            Some(Probe::Gone) => return StepResult::Gone,
            Some(Probe::Running | Probe::Exited(_)) => break,
            None => return unexpected(&lines),
        }
    }
    follow_existing(transport, run_id, key, output, &mut reattaches).await
}

async fn follow_existing<T: Transport>(
    transport: &T,
    run_id: &str,
    key: &str,
    output: &mpsc::UnboundedSender<String>,
    reattaches: &mut usize,
) -> StepResult {
    // After a reconnect, observe again before probing so output written
    // while disconnected is still shown.
    let mut seen = 0;
    loop {
        let observed = observe(transport, run_id, key, &mut seen, output).await;
        if let Err(reason) = reconnect_if_lost(transport, &observed, reattaches).await {
            return StepResult::Interrupted(reason);
        }
        if observed.is_err() {
            continue;
        }
        let (result, lines) = run_collect(transport, &probe_script(run_id, key)).await;
        if let Err(reason) = reconnect_if_lost(transport, &result, reattaches).await {
            return StepResult::Interrupted(reason);
        }
        if result.is_err() {
            continue;
        }
        match parse_probe(&lines) {
            Some(Probe::Exited(code)) => return StepResult::Exited(code),
            Some(Probe::Gone) => return StepResult::Gone,
            // The observer returns only once the step's process has ended.
            Some(Probe::Running | Probe::NotStarted) => {
                *reattaches += 1;
                if *reattaches > MAX_REATTACHES {
                    return StepResult::Interrupted("could not follow the step's output".into());
                }
            }
            None => return unexpected(&lines),
        }
    }
}

fn unexpected(lines: &[String]) -> StepResult {
    StepResult::Interrupted(format!(
        "unexpected status from the server: {}",
        lines.join("\n")
    ))
}

pub(crate) async fn run_collect<T: Transport>(
    transport: &T,
    script: &str,
) -> (Result<i32, TransportError>, Vec<String>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let result = transport.run(script, tx).await;
    let mut lines = Vec::new();
    while let Some(line) = rx.recv().await {
        lines.push(line);
    }
    (result, lines)
}

/// Forwards log lines after the first `seen` to `output`, counting them.
async fn observe<T: Transport>(
    transport: &T,
    run_id: &str,
    key: &str,
    seen: &mut usize,
    output: &mpsc::UnboundedSender<String>,
) -> Result<i32, TransportError> {
    let script = observe_script(run_id, key, *seen);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let forward = async {
        let mut started = false;
        while let Some(line) = rx.recv().await {
            if !started {
                started = line == LOG_START;
                continue;
            }
            *seen += 1;
            let _ = output.send(line);
        }
    };
    let (result, ()) = tokio::join!(transport.run(&script, tx), forward);
    result
}

/// On a transport error, reconnects with backoff. `Ok` when there was no
/// error or the connection is back.
async fn reconnect_if_lost<T: Transport>(
    transport: &T,
    result: &Result<i32, TransportError>,
    reattaches: &mut usize,
) -> Result<(), String> {
    let Err(err) = result else {
        return Ok(());
    };
    *reattaches += 1;
    if *reattaches > MAX_REATTACHES {
        return Err(err.to_string());
    }
    let mut last = err.to_string();
    for delay in RECONNECT_DELAYS {
        tokio::time::sleep(delay).await;
        match transport.reconnect().await {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_refuses_to_start_a_step_twice() {
        let s = launch_script("r1", "step-2", "true");
        let guard = s
            .find("mkdir \"$HOME\"/.shipslip/runs/'r1'/step-2\n")
            .unwrap();
        assert!(guard < s.find("setsid").unwrap());
        assert!(s.starts_with("set -e\n"));
    }

    #[test]
    fn launch_embeds_the_script_quoted() {
        let s = launch_script("r1", "step-0", "echo 'hi' \"$HOME\"");
        assert!(s.contains(r#"printf '%s' 'echo '\''hi'\'' "$HOME"' >"#));
    }

    #[test]
    fn observe_resumes_after_seen_lines() {
        assert!(observe_script("r1", "step-1", 0).contains("tail -n +1 "));
        assert!(observe_script("r1", "step-1", 7).contains("tail -n +8 "));
    }

    #[test]
    fn probe_output_is_parsed_from_the_last_line() {
        let lines = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            parse_probe(&lines(&["motd", "exited 3"])),
            Some(Probe::Exited(3))
        );
        assert_eq!(parse_probe(&lines(&["running", ""])), Some(Probe::Running));
        assert_eq!(parse_probe(&lines(&["gone"])), Some(Probe::Gone));
        assert_eq!(
            parse_probe(&lines(&["not-started"])),
            Some(Probe::NotStarted)
        );
        assert_eq!(parse_probe(&lines(&["exited x"])), None);
        assert_eq!(parse_probe(&[]), None);
    }
}
