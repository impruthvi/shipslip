//! The Cloud API over the user's `curl`. The URL and the token reach curl
//! only through its config on stdin, never as arguments.

use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use zeroize::{Zeroize, Zeroizing};

use super::{CloudApi, CloudError, CloudToken, API_HOST};
use crate::receipt::cloud::is_plain_id;

const STATUS_MARK: &str = "\n@shipslip-http ";

/// Token variables no child process should inherit.
const TOKEN_ENV: &[&str] = &["LARAVEL_CLOUD_TOKEN", "LARAVEL_CLOUD_API_TOKEN"];

pub struct CurlCloudApi {
    token: CloudToken,
    program: String,
}

impl CurlCloudApi {
    pub fn new(token: CloudToken) -> Self {
        Self {
            token,
            program: "curl".into(),
        }
    }

    #[cfg(test)]
    fn with_program(token: CloudToken, program: &str) -> Self {
        Self {
            token,
            program: program.into(),
        }
    }

    async fn get(&self, path: &str) -> Result<String, CloudError> {
        let mut command = Command::new(&self.program);
        for key in TOKEN_ENV {
            command.env_remove(key);
        }
        // --disable MUST be first: ignore curlrc files that could enable
        // tracing, redirects or insecure TLS.
        command
            .args([
                "--disable",
                "--config",
                "-",
                "--silent",
                "--proto",
                "=https",
                "--max-time",
                "60",
                "--connect-timeout",
                "10",
                "--max-filesize",
                "16777216",
                "--write-out",
                "\n@shipslip-http %{http_code}",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| CloudError::CouldNotConnect)?;
        let config = Zeroizing::new(format!(
            "url = \"https://{API_HOST}/api{path}\"\n\
             header = \"Authorization: Bearer {}\"\n\
             header = \"Accept: application/vnd.api+json, application/json\"\n\
             user-agent = \"Shipslip/{}\"\n",
            self.token.expose_secret(),
            env!("CARGO_PKG_VERSION")
        ));
        let mut stdin = child.stdin.take().expect("piped stdin");
        let sent = stdin.write_all(config.as_bytes()).await;
        drop(stdin);
        drop(config);
        sent.map_err(|_| CloudError::CouldNotConnect)?;
        let mut output = child
            .wait_with_output()
            .await
            .map_err(|_| CloudError::ConnectionLost)?;
        let result = response(output.status.code(), &output.stdout);
        output.stdout.zeroize();
        result
    }
}

fn response(exit: Option<i32>, stdout: &[u8]) -> Result<String, CloudError> {
    match exit {
        Some(0) => {}
        Some(6 | 7 | 35 | 60) => return Err(CloudError::CouldNotConnect),
        Some(63) => return Err(CloudError::Unreadable),
        _ => return Err(CloudError::ConnectionLost),
    }
    let text = std::str::from_utf8(stdout).map_err(|_| CloudError::Unreadable)?;
    let (body, status) = text
        .rsplit_once(STATUS_MARK)
        .ok_or(CloudError::Unreadable)?;
    let status: u16 = status.trim().parse().map_err(|_| CloudError::Unreadable)?;
    match status {
        200 => Ok(body.to_string()),
        401 => Err(CloudError::TokenRejected),
        403 => Err(CloudError::PermissionMissing),
        404 => Err(CloudError::NotFound),
        429 => Err(CloudError::RateLimited),
        status => Err(CloudError::Status(status)),
    }
}

fn plain_id(id: &str) -> Result<&str, CloudError> {
    if is_plain_id(id) {
        Ok(id)
    } else {
        Err(CloudError::NotFound)
    }
}

impl CloudApi for CurlCloudApi {
    async fn list_deployments(&self, environment: &str, page: u32) -> Result<String, CloudError> {
        let environment = plain_id(environment)?;
        self.get(&format!(
            "/environments/{environment}/deployments?page={page}"
        ))
        .await
    }

    async fn get_deployment(&self, deployment: &str) -> Result<String, CloudError> {
        self.get(&format!("/deployments/{}", plain_id(deployment)?))
            .await
    }

    async fn deployment_logs(&self, deployment: &str) -> Result<String, CloudError> {
        self.get(&format!("/deployments/{}/logs", plain_id(deployment)?))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::receipt::tests::TempRoot;

    const TOKEN: &str = "7|cloudSECRETtoken";

    /// A stand-in curl that saves its arguments, environment and config, then
    /// answers with `status` and `body`.
    fn fake_curl(root: &TempRoot, status: &str, exit: i32) -> String {
        let program = root.0.join("curl");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{dir}/args'\nenv > '{dir}/env'\n\
                 cat > '{dir}/config'\nprintf '{{\"data\": []}}\\n@shipslip-http {status}'\nexit {exit}\n",
                dir = root.0.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        program.to_string_lossy().into_owned()
    }

    fn api(program: &str) -> CurlCloudApi {
        CurlCloudApi::with_program(CloudToken::new(TOKEN.into()).unwrap(), program)
    }

    #[tokio::test]
    async fn the_token_and_url_go_only_through_stdin_to_the_fixed_host() {
        let root = TempRoot::new();
        let program = fake_curl(&root, "200", 0);
        std::env::set_var("LARAVEL_CLOUD_TOKEN", "ambient-token");
        let body = api(&program)
            .list_deployments("env-9f2c41d0", 1)
            .await
            .unwrap();
        assert_eq!(body, "{\"data\": []}");
        let args = std::fs::read_to_string(root.0.join("args")).unwrap();
        assert!(args.starts_with("--disable\n"), "{args}");
        assert!(
            !args.contains(TOKEN) && !args.contains("cloud.laravel.com"),
            "{args}"
        );
        let env = std::fs::read_to_string(root.0.join("env")).unwrap();
        assert!(!env.contains("ambient-token"));
        let config = std::fs::read_to_string(root.0.join("config")).unwrap();
        assert!(config.contains(
            "url = \"https://cloud.laravel.com/api/environments/env-9f2c41d0/deployments?page=1\""
        ));
        assert!(config.contains(&format!("Authorization: Bearer {TOKEN}")));
    }

    #[tokio::test]
    async fn http_statuses_and_curl_failures_become_typed_errors() {
        for (status, exit, expected) in [
            ("401", 0, CloudError::TokenRejected),
            ("403", 0, CloudError::PermissionMissing),
            ("404", 0, CloudError::NotFound),
            ("429", 0, CloudError::RateLimited),
            ("500", 0, CloudError::Status(500)),
            ("xyz", 0, CloudError::Unreadable),
            ("200", 6, CloudError::CouldNotConnect),
            ("200", 7, CloudError::CouldNotConnect),
            ("200", 35, CloudError::CouldNotConnect),
            ("200", 60, CloudError::CouldNotConnect),
            ("200", 63, CloudError::Unreadable),
            ("200", 18, CloudError::ConnectionLost),
            ("200", 28, CloudError::ConnectionLost),
        ] {
            let root = TempRoot::new();
            let program = fake_curl(&root, status, exit);
            let error = api(&program).get_deployment("depl-1").await.unwrap_err();
            assert_eq!(error, expected, "{status} {exit}");
            assert!(!error.to_string().contains(TOKEN));
        }
    }

    #[test]
    fn a_body_that_is_not_utf8_or_lacks_the_status_is_unreadable() {
        assert_eq!(
            response(Some(0), b"\xff\n@shipslip-http 200"),
            Err(CloudError::Unreadable)
        );
        assert_eq!(response(Some(0), b"{}"), Err(CloudError::Unreadable));
    }

    #[tokio::test]
    async fn a_missing_curl_is_reported_as_no_connection() {
        let root = TempRoot::new();
        let missing = root.0.join("no-curl").to_string_lossy().into_owned();
        assert_eq!(
            api(&missing).get_deployment("depl-1").await.unwrap_err(),
            CloudError::CouldNotConnect
        );
    }

    #[tokio::test]
    async fn an_id_that_could_change_the_path_is_never_sent() {
        let root = TempRoot::new();
        let program = fake_curl(&root, "200", 0);
        for id in ["../me", "a/b", "a?b", ""] {
            assert_eq!(
                api(&program).deployment_logs(id).await.unwrap_err(),
                CloudError::NotFound
            );
        }
        assert!(!root.0.join("args").exists());
    }
}
