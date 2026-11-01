//! No-change proof through the MCP CLI's stdio boundary and a real sshd.

use super::*;
use serde_json::{json, Value};
use shipslip::config::{approve_trust, LoadedConfig};
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

struct Session {
    child: Child,
    input: Option<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
}

impl Session {
    fn start(local: &Path, home: &Path, bin: &Path) -> Self {
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_slip"))
            .args(["mcp", "--read-logs"])
            .current_dir(local)
            .env("HOME", home)
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env_remove("SHIPSLIP_CONFIG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
        }
    }

    async fn send(&mut self, name: &str, arguments: Value) {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":name,"arguments":arguments,"_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientCapabilities":{}
            }
        }});
        self.input
            .as_mut()
            .unwrap()
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn receive(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(30), self.output.next_line())
            .await
            .expect("MCP did not answer")
            .unwrap()
            .expect("MCP exited before answering");
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert!(response.get("error").is_none(), "{response}");
        let result = &response["result"];
        assert_eq!(
            serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
            result["structuredContent"]
        );
        result.clone()
    }

    async fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.send(name, arguments).await;
        self.receive().await
    }

    async fn finish(mut self) {
        drop(self.input.take());
        assert!(
            tokio::time::timeout(Duration::from_secs(10), self.output.next_line())
                .await
                .expect("MCP did not close stdout")
                .unwrap()
                .is_none()
        );
        let output = tokio::time::timeout(Duration::from_secs(10), self.child.wait_with_output())
            .await
            .expect("MCP did not exit on EOF")
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

struct Home(PathBuf);

impl Home {
    fn new(server: &Server) -> Self {
        // OpenSSH's control socket must fit macOS's Unix socket path limit.
        let path = PathBuf::from("/tmp").join(format!(
            "{}-mcp",
            server.dir.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn receipts(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.0.join("Library/Application Support/Shipslip/receipts")
        } else {
            self.0.join(".local/share/shipslip/receipts")
        }
    }

    fn trust(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.0
                .join("Library/Application Support/Shipslip/trust.json")
        } else {
            self.0.join(".config/shipslip/trust.json")
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct LocalFile {
    path: PathBuf,
    content: Option<Vec<u8>>,
    inode: u64,
    size: u64,
    modified: (i64, i64),
}
fn local_files(root: &Path) -> Vec<LocalFile> {
    fn visit(path: &Path, files: &mut Vec<LocalFile>) {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).unwrap();
        files.push(LocalFile {
            path: path.into(),
            content: meta.is_file().then(|| std::fs::read(path).unwrap()),
            inode: meta.ino(),
            size: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
        });
        if meta.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                visit(&entry.unwrap().path(), files);
            }
        }
    }
    let mut files = Vec::new();
    visit(root, &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

// Installed before recording the remote tree. The driver arms this outside
// that tree, then waits until the log-data pipeline is running before cutting SSH.
const READ_GATE: &str = r#"gzip() {
  if [ -e /tmp/mcp-drop-read ]; then
    : > /tmp/mcp-read-started
    sleep 30
  fi
  command gzip "$@"
}
"#;

async fn wait_for_read(server: &Server) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while server.exec("test -e /tmp/mcp-read-started && echo yes || echo no", "") != "yes" {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("MCP did not reach the remote log-data read");
}

#[tokio::test]
async fn mcp_tools_change_no_remote_app_git_log_or_local_record_files() {
    let server = Server::start().await;
    let path = server.logs_app("mcp-read-only");
    server.exec(&format!("mkdir -p {path}/.git/shipslip.lock; printf held > {path}/.git/shipslip.lock/owner; printf marker > {path}/.git/shipslip.last-run"), "");
    server.exec("cat > ~/.bash_profile", READ_GATE);
    let local = server.dir.join("mcp-local");
    let home = Home::new(&server);
    let bin = server.dir.join("mcp-bin");
    std::fs::create_dir_all(&bin).unwrap();
    // Only route the real OpenSSH client to this test's isolated ssh config.
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nexec /usr/bin/ssh -F {} \"$@\"\n",
            quote(&server.config().to_string_lossy())
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir_all(local.join(".git")).unwrap();
    std::fs::write(local.join(".shipslip.toml"), format!("[project]\nname='fixture'\nstack='laravel'\n[env.staging]\nssh='server'\npath='{path}'\nbranch='main'\nproduction=false\n[env.unapproved]\nssh='server'\npath='{path}'\nbranch='main'\nproduction=false\n[recipe.deploy]\nsteps=['true']\n")).unwrap();
    let config = LoadedConfig::load(&local, None).unwrap();
    approve_trust(
        &home.trust(),
        config.repo_root(),
        "staging",
        config.trust_snapshot("staging").unwrap(),
    )
    .unwrap();
    let receipts = home.receipts().join("fixture/staging");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(receipts.join("1000-abcd1234-0.json"), json!({
        "version":1,"project":"fixture","repo_root":config.repo_root(),"run_id":"abcd1234-0","owner_pid":1,
        "confirmed":true,"started_at_ms":1000,"finished_at_ms":2000,"status":"Final","phase":"Finished",
        "target":config.target("staging").unwrap(),"run_plan":"Deploy","from_sha":"a".repeat(40),"target_sha":"b".repeat(40),
        "commits":[],"recipe_hash":"c".repeat(64),"steps":[],"maintenance_down_done":false,"maintenance_up_done":false,
        "app_left_down":false,"mutation_started":true,"outcome":"Succeeded"
    }).to_string()).unwrap();
    // Reads may update atime, so exclude it. Include contents,
    // directory entries, inode, size, mtime, ctime, permissions and symlink targets.
    let inspect = "find /home/deploy -type f -exec sha256sum {} + | sort; find /home/deploy -printf '%p %y %m %s %i %T@ %C@ %l\\n' | sort";
    let remote_before = server.exec(inspect, "");
    let local_records = || {
        (
            local_files(&local),
            local_files(&home.receipts()),
            local_files(&home.trust()),
        )
    };
    let local_before = local_records();
    let mut session = Session::start(&local, &home.0, &bin);
    for (name, args) in [
        ("status", json!({})),
        ("receipts", json!({"project":"fixture"})),
        ("receipt", json!({"project":"fixture","id":"abcd1234"})),
    ] {
        let result = session.call(name, args).await;
        assert_eq!(result["isError"], false, "{name}: {result}");
    }
    let args = json!({"checkout":config.repo_root(),"environment":"staging"});
    let summary = session.call("logs", args.clone()).await;
    assert_eq!(summary["isError"], false, "{summary}");
    let groups = summary["structuredContent"]["groups"].as_array().unwrap();
    assert!(!groups.is_empty());
    let mut detail_args = args.clone();
    detail_args["id"] = groups[0]["full_id"].clone();
    let detail = session.call("log_group", detail_args.clone()).await;
    assert_eq!(detail["isError"], false, "{detail}");
    assert_eq!(
        detail["structuredContent"]["group"]["full_id"],
        groups[0]["full_id"]
    );
    assert!(detail["structuredContent"].get("groups").is_none());
    assert!(detail["structuredContent"].get("overflow_groups").is_none());
    assert_eq!(server.exec(inspect, ""), remote_before);
    assert_eq!(local_records(), local_before);

    for (name, mut args) in [("logs", args.clone()), ("log_group", detail_args.clone())] {
        args["environment"] = json!("unapproved");
        let result = session.call(name, args).await;
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(
            result["structuredContent"]["error"]["kind"], "not_approved",
            "{result}"
        );
    }
    assert_eq!(server.exec(inspect, ""), remote_before);
    assert_eq!(local_records(), local_before);

    for (name, args) in [("logs", args), ("log_group", detail_args)] {
        server.exec("rm -f /tmp/mcp-read-started; touch /tmp/mcp-drop-read", "");
        session.send(name, args).await;
        wait_for_read(&server).await;
        server.exec_as("root", "pkill -f 'sshd: deploy'", "");
        let dropped = session.receive().await;
        assert_eq!(dropped["isError"], true, "{dropped}");
        assert_eq!(
            dropped["structuredContent"]["error"]["kind"], "connection_lost",
            "{dropped}"
        );
        server.exec_as(
            "root",
            "pkill -u deploy sleep || true; rm -f /tmp/mcp-drop-read /tmp/mcp-read-started",
            "",
        );
        assert_eq!(server.exec(inspect, ""), remote_before);
        assert_eq!(local_records(), local_before);
    }
    // A failed read must not terminate the MCP server or poison local tools.
    let status = session.call("status", json!({})).await;
    assert_eq!(status["isError"], false, "{status}");
    session.finish().await;
    assert_eq!(server.exec(inspect, ""), remote_before);
    assert_eq!(local_records(), local_before);
}
