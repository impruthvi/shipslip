//! Pulling a Cloud target's deployments into receipts (ADR 0006). Shipslip
//! only reads Laravel Cloud: every request here is a GET.

use std::fmt;
use std::future::Future;
use std::path::Path;

use serde::Deserialize;
use zeroize::Zeroizing;

use crate::config::ApprovedCloudTarget;
use crate::receipt::cloud::{
    find_cloud, is_plain_id, write_cloud, CloudKind, CloudReceipt, CloudReportedTimes,
    CLOUD_VERSION,
};
use crate::receipt::{now_ms, ReceiptError};

mod curl;

pub use curl::CurlCloudApi;

/// The only host a Cloud token is sent to. No setting changes it.
pub const API_HOST: &str = "cloud.laravel.com";

/// How many of the newest deployments a pull reads.
pub const PULL_LIMIT: usize = 10;

/// The status Laravel Cloud reports for a deployment that went live.
pub const SUCCEEDED: &str = "deployment.succeeded";

/// A Laravel Cloud API token, held for one pull. Shipslip never stores it,
/// and debug formatting never reveals it.
pub struct CloudToken(Zeroizing<String>);

impl CloudToken {
    /// One line of printable ASCII without quotes or backslashes, so it fits
    /// a quoted curl config value unchanged.
    pub fn new(token: String) -> Result<Self, InvalidCloudToken> {
        let token = Zeroizing::new(token);
        if token.is_empty()
            || token.len() > 4096
            || !token
                .bytes()
                .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
        {
            return Err(InvalidCloudToken);
        }
        Ok(Self(token))
    }

    pub(crate) fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CloudToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CloudToken([REDACTED])")
    }
}

#[derive(Debug, thiserror::Error)]
#[error(
    "a Laravel Cloud token is one line of letters, digits and symbols, without spaces or quotes"
)]
pub struct InvalidCloudToken;

/// Why a read from Laravel Cloud failed. None of these carry response text
/// or the token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloudError {
    #[error("Laravel Cloud rejected the token; it may be wrong, expired or revoked")]
    TokenRejected,
    #[error("the token lacks a permission this read needs; give it `environment.view`")]
    PermissionMissing,
    #[error("Laravel Cloud has no such environment or deployment, or the token cannot see it")]
    NotFound,
    #[error("Laravel Cloud is rate limiting this token; try again in a minute")]
    RateLimited,
    #[error("Laravel Cloud sent a response Shipslip could not read")]
    Unreadable,
    #[error("could not connect to {API_HOST}; check your network")]
    CouldNotConnect,
    #[error("the connection to {API_HOST} was lost before the response ended")]
    ConnectionLost,
    #[error("Laravel Cloud answered with HTTP {0}")]
    Status(u16),
}

/// The Cloud API boundary: three reads, each returning the response body.
/// Implementations hold the token and talk only to [`API_HOST`].
pub trait CloudApi {
    fn list_deployments(
        &self,
        environment: &str,
        page: u32,
    ) -> impl Future<Output = Result<String, CloudError>> + Send;

    fn get_deployment(
        &self,
        deployment: &str,
    ) -> impl Future<Output = Result<String, CloudError>> + Send;

    fn deployment_logs(
        &self,
        deployment: &str,
    ) -> impl Future<Output = Result<String, CloudError>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullEvent {
    Listing {
        cloud_environment: String,
    },
    Written {
        deployment_id: String,
        cloud_status: String,
    },
    AlreadyHeld {
        deployment_id: String,
    },
    /// Not finished or not succeeded; this version writes receipts only for
    /// succeeded deployments.
    Skipped {
        deployment_id: String,
        cloud_status: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PullReport {
    pub written: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum PullError {
    #[error(transparent)]
    Cloud(#[from] CloudError),
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
}

#[derive(Deserialize)]
struct DeploymentPage {
    data: Vec<Deployment>,
}

#[derive(Deserialize)]
struct Deployment {
    id: String,
    attributes: Attributes,
}

#[derive(Deserialize)]
struct Attributes {
    status: String,
    branch_name: String,
    commit_hash: String,
    commit_message: String,
    commit_author: Option<String>,
    failure_reason: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
}

/// Reads the newest deployments of `target` and writes a receipt for each
/// succeeded one not already held. Each receipt is written durably before
/// the next, so a pull that stops part way leaves whole receipts only.
pub async fn pull(
    target: &ApprovedCloudTarget,
    api: &impl CloudApi,
    receipts_root: &Path,
    mut events: impl FnMut(PullEvent),
) -> Result<PullReport, PullError> {
    let cloud = target.target();
    events(PullEvent::Listing {
        cloud_environment: cloud.cloud_environment.clone(),
    });
    let body = api.list_deployments(&cloud.cloud_environment, 1).await?;
    let page: DeploymentPage = serde_json::from_str(&body).map_err(|_| CloudError::Unreadable)?;
    let newest: Vec<Deployment> = page.data.into_iter().take(PULL_LIMIT).collect();
    // Check the whole page before writing, so a bad entry writes nothing.
    if newest.iter().any(|deployment| !is_plain_id(&deployment.id)) {
        return Err(CloudError::Unreadable.into());
    }
    let mut report = PullReport::default();
    for deployment in newest {
        let Deployment { id, attributes } = deployment;
        if attributes.status != SUCCEEDED {
            events(PullEvent::Skipped {
                deployment_id: id,
                cloud_status: attributes.status,
            });
            continue;
        }
        if find_cloud(receipts_root, target.project(), &cloud.env, &id)?.is_some() {
            events(PullEvent::AlreadyHeld { deployment_id: id });
            continue;
        }
        let receipt = CloudReceipt {
            version: CLOUD_VERSION,
            kind: CloudKind::Cloud,
            project: target.project().to_string(),
            env: cloud.env.clone(),
            cloud_environment: cloud.cloud_environment.clone(),
            deployment_id: id.clone(),
            cloud_status: attributes.status.clone(),
            branch: attributes.branch_name,
            commit_hash: attributes.commit_hash,
            commit_message: attributes.commit_message,
            commit_author: attributes.commit_author,
            failure_reason: attributes.failure_reason,
            reported: CloudReportedTimes {
                started_at: attributes.started_at,
                finished_at: attributes.finished_at,
            },
            pulled_at_ms: now_ms(),
        };
        write_cloud(receipts_root, &receipt)?;
        report.written += 1;
        events(PullEvent::Written {
            deployment_id: id,
            cloud_status: attributes.status,
        });
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
