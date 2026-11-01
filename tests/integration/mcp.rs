use super::*;
use serde_json::{json, Value};
use shipslip::config::{approve_trust, LoadedConfig};
use shipslip::read_only::{Connector, Reader};

#[allow(dead_code)]
#[path = "../../src/bin/slip/mcp.rs"]
mod protocol;

struct SshConnector {
    config: PathBuf,
    drop_connection: bool,
}
impl Connector for SshConnector {
    type Connection = ReadTransport;
    async fn connect(&self, alias: &str) -> Result<ReadTransport, TransportError> {
        Ok(ReadTransport {
            ssh: SshTransport::connect_with_config(alias, &self.config).await?,
            drop_connection: self.drop_connection,
        })
    }
}
struct ReadTransport {
    ssh: SshTransport,
    drop_connection: bool,
}
impl Transport for ReadTransport {
    async fn run(
        &self,
        script: &str,
        output: mpsc::Sender<String>,
        limit: usize,
    ) -> Result<i32, TransportError> {
        self.ssh
            .run(
                if self.drop_connection {
                    "kill -9 $$"
                } else {
                    script
                },
                output,
                limit,
            )
            .await
    }
    async fn reconnect(&self) -> Result<(), TransportError> {
        panic!("reads must not reconnect")
    }
}
fn request(name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments,"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}})
}
async fn call(server: &Server, local: &Path, request: Value, drop_connection: bool) -> Value {
    let reader = Reader::with_connector(
        local.join("receipts"),
        local.join("trust.json"),
        SshConnector {
            config: server.config(),
            drop_connection,
        },
    );
    let input = format!("{request}\n");
    let mut output = Vec::new();
    protocol::serve(reader, true, input.as_bytes(), &mut output)
        .await
        .unwrap();
    serde_json::from_slice(&output).unwrap()
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

#[tokio::test]
async fn mcp_tools_change_no_remote_app_git_log_or_local_record_files() {
    let server = Server::start().await;
    let path = server.logs_app("mcp-read-only");
    server.exec(&format!("mkdir -p {path}/.git/shipslip.lock; printf held > {path}/.git/shipslip.lock/owner; printf marker > {path}/.git/shipslip.last-run"), "");
    let local = server.dir.join("mcp-local");
    std::fs::create_dir_all(local.join(".git")).unwrap();
    std::fs::write(local.join(".shipslip.toml"), format!("[project]\nname='fixture'\nstack='laravel'\n[env.staging]\nssh='server'\npath='{path}'\nbranch='main'\nproduction=false\n[recipe.deploy]\nsteps=['true']\n")).unwrap();
    let config = LoadedConfig::load(&local, None).unwrap();
    approve_trust(
        &local.join("trust.json"),
        config.repo_root(),
        "staging",
        config.trust_snapshot("staging").unwrap(),
    )
    .unwrap();
    let receipts = local.join("receipts/fixture/staging");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(receipts.join("1000-abcd1234-0.json"), json!({
        "version":1,"project":"fixture","repo_root":config.repo_root(),"run_id":"abcd1234-0","owner_pid":1,
        "confirmed":true,"started_at_ms":1000,"finished_at_ms":2000,"status":"Final","phase":"Finished",
        "target":config.target("staging").unwrap(),"run_plan":"Deploy","from_sha":"a".repeat(40),"target_sha":"b".repeat(40),
        "commits":[],"recipe_hash":"c".repeat(64),"steps":[],"maintenance_down_done":false,"maintenance_up_done":false,
        "app_left_down":false,"mutation_started":true,"outcome":"Succeeded"
    }).to_string()).unwrap();
    let inspect = "find /home/deploy -type f -print0 | sort -z | xargs -0 sha256sum; find /home/deploy -printf '%p %s %i %T@ %C@\\n' | sort";
    let remote_before = server.exec(inspect, "");
    let local_before = local_files(&local);
    for (name, args) in [
        ("status", json!({})),
        ("receipts", json!({"project":"fixture"})),
        ("receipt", json!({"project":"fixture","id":"abcd1234"})),
    ] {
        let response = call(&server, &local, request(name, args), false).await;
        assert_eq!(response["result"]["isError"], false, "{response}");
    }
    let args = json!({"checkout":config.repo_root(),"environment":"staging"});
    let summary = call(&server, &local, request("logs", args.clone()), false).await;
    assert_eq!(summary["result"]["isError"], false, "{summary}");
    let groups = summary["result"]["structuredContent"]["groups"]
        .as_array()
        .unwrap();
    assert!(!groups.is_empty());
    let mut detail_args = args.clone();
    detail_args["id"] = groups[0]["full_id"].clone();
    let detail = call(&server, &local, request("log_group", detail_args), false).await;
    assert_eq!(detail["result"]["isError"], false, "{detail}");
    let dropped = call(&server, &local, request("logs", args.clone()), true).await;
    assert_eq!(
        dropped["result"]["structuredContent"]["error"]["kind"],
        "connection_lost"
    );
    assert_eq!(server.exec(inspect, ""), remote_before);
    assert_eq!(local_files(&local), local_before);
    // Edit config as the test driver; neither MCP refusal may write it or connect.
    let config_path = local.join(".shipslip.toml");
    std::fs::write(
        &config_path,
        std::fs::read_to_string(&config_path)
            .unwrap()
            .replace("ssh='server'", "ssh='never-connect'"),
    )
    .unwrap();
    let local_before = local_files(&local);
    for name in ["logs", "log_group"] {
        let mut args = args.clone();
        if name == "log_group" {
            args["id"] = json!("1");
        }
        let response = call(&server, &local, request(name, args), false).await;
        assert_eq!(
            response["result"]["structuredContent"]["error"]["kind"],
            "not_approved"
        );
    }
    assert_eq!(server.exec(inspect, ""), remote_before);
    assert_eq!(local_files(&local), local_before);
}
