//! The MCP server's only library boundary: records from this Mac and log
//! snapshots for approved environments.

use crate::config::LoadedConfig;
use crate::logs;
pub use crate::logs::{Level, Since};
use crate::transport::{Transport, TransportError};
use crate::DeployTarget;
use std::future::Future;
use std::path::{Path, PathBuf};

use crate::records::{self, ReceiptList, ReceiptRecord, Status};
pub use crate::records::{versioned, versioned_error, ReceiptQueryError};

/// A connection factory, injectable at the transport boundary for protocol tests.
/// Connections require a target checked by this module, not an arbitrary alias.
///
/// ```compile_fail,E0308
/// use shipslip::read_only::{Connector, SshConnector};
///
/// let _connection = SshConnector.connect("unapproved-host");
/// ```
pub trait Connector {
    type Connection: Transport;
    fn connect(
        &self,
        target: &ApprovedTarget,
    ) -> impl Future<Output = Result<Self::Connection, TransportError>> + Send;
}

pub struct SshConnector;

#[cfg(unix)]
impl Connector for SshConnector {
    type Connection = crate::transport::SshTransport;
    async fn connect(&self, target: &ApprovedTarget) -> Result<Self::Connection, TransportError> {
        crate::transport::SshTransport::connect(target.ssh_alias()).await
    }
}

/// A target checked against both the release map and the current approval.
/// It has no public constructor and carries no mutation capability.
///
/// ```compile_fail,E0423
/// use shipslip::read_only::ApprovedTarget;
///
/// fn forge(target: shipslip::DeployTarget) -> ApprovedTarget {
///     ApprovedTarget(target)
/// }
/// ```
pub struct ApprovedTarget(DeployTarget);

impl ApprovedTarget {
    pub fn ssh_alias(&self) -> &str {
        &self.0.ssh_alias
    }
}

#[derive(Debug, Default)]
pub struct LogQuery {
    pub since: Option<Since>,
    pub level: Option<Level>,
    pub text: Option<String>,
}

pub use crate::records::{
    limit_log_text, limit_record_text, LogsDetail, LogsQueryError, LogsSummary,
};

pub struct Reader<C = SshConnector> {
    receipts_root: PathBuf,
    trust_path: PathBuf,
    connector: C,
}

impl Reader {
    pub fn new(receipts_root: impl Into<PathBuf>, trust_path: impl Into<PathBuf>) -> Self {
        Self::with_connector(receipts_root, trust_path, SshConnector)
    }
}

impl<C> Reader<C> {
    pub fn with_connector(
        receipts_root: impl Into<PathBuf>,
        trust_path: impl Into<PathBuf>,
        connector: C,
    ) -> Self {
        Self {
            receipts_root: receipts_root.into(),
            trust_path: trust_path.into(),
            connector,
        }
    }

    pub fn status(&self) -> Result<Status, ReceiptQueryError> {
        records::status(&self.receipts_root, &self.trust_path)
    }

    pub fn receipts(
        &self,
        project: &str,
        environment: Option<&str>,
        limit: Option<usize>,
    ) -> Result<ReceiptList, ReceiptQueryError> {
        records::receipts(
            &self.receipts_root,
            project,
            environment,
            Some(limit.unwrap_or(20)),
            None,
        )
    }

    pub fn receipt(&self, project: &str, id: &str) -> Result<ReceiptRecord, ReceiptQueryError> {
        records::receipt(&self.receipts_root, project, id)
    }
}

impl<C: Connector> Reader<C> {
    fn approved_target(
        &self,
        checkout: &Path,
        environment: &str,
    ) -> Result<ApprovedTarget, LogsQueryError> {
        let invalid = |message: String| LogsQueryError::invalid(message);
        let checkout = std::fs::canonicalize(checkout)
            .map_err(|_| invalid("checkout is not listed in the release map".into()))?;
        let map = self
            .status()
            .map_err(|error| invalid(format!("cannot read release map: {error:?}")))?;
        if !map
            .repos
            .iter()
            .any(|repo| Path::new(repo.repo_root.raw()) == checkout)
        {
            return Err(invalid("checkout is not listed in the release map".into()));
        }
        let not_approved = || LogsQueryError::NotApproved {
            environment: environment.into(),
        };
        let config = LoadedConfig::load(&checkout, Some(&checkout.join(".shipslip.toml")))
            .map_err(|_| not_approved())?;
        if config.repo_root() != checkout {
            return Err(invalid("checkout must be the repository root".into()));
        }
        if config.target(environment).is_none() {
            return Err(invalid(format!(
                "environment `{environment}` is not in the config"
            )));
        }
        let target = config
            .trusted_target(environment, &self.trust_path)
            .map_err(|_| not_approved())?;
        Ok(ApprovedTarget(target.target().clone()))
    }

    async fn snapshot(
        &self,
        checkout: &Path,
        environment: &str,
        query: &LogQuery,
    ) -> Result<logs::Snapshot, LogsQueryError> {
        let approved = self.approved_target(checkout, environment)?;
        let transport = self.connector.connect(&approved).await?;
        let request = logs::Request {
            since: query.since.clone(),
            allow_outside: true,
            ..logs::Request::default()
        };
        logs::snapshot(&transport, &approved.0, &request)
            .await
            .map_err(Into::into)
    }

    pub async fn logs(
        &self,
        checkout: &Path,
        environment: &str,
        query: &LogQuery,
    ) -> Result<LogsSummary, LogsQueryError> {
        let snapshot = self.snapshot(checkout, environment, query).await?;
        let level = query.level.unwrap_or(Level::Error);
        let groups = logs::group(&snapshot, level, query.text.as_deref());
        Ok(records::logs_summary(
            &snapshot,
            &groups,
            level,
            query.text.as_deref(),
        ))
    }

    pub async fn log_group(
        &self,
        checkout: &Path,
        environment: &str,
        id: &str,
        query: &LogQuery,
    ) -> Result<LogsDetail, LogsQueryError> {
        let snapshot = self.snapshot(checkout, environment, query).await?;
        let level = query.level.unwrap_or(Level::Error);
        let groups = logs::group(&snapshot, level, query.text.as_deref());
        match logs::find(&groups, id) {
            logs::Lookup::Found(group) => Ok(records::logs_detail(
                &snapshot,
                group,
                level,
                query.text.as_deref(),
            )),
            logs::Lookup::Missing => Err(LogsQueryError::Missing),
            logs::Lookup::Ambiguous(groups) => Err(LogsQueryError::Ambiguous {
                ids: groups.iter().map(|group| group.id.clone().into()).collect(),
            }),
        }
    }
}
