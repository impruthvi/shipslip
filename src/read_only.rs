//! The MCP server's only library boundary: records already held on this Mac.

use std::path::PathBuf;

use crate::records::{self, ReceiptList, ReceiptRecord, Status};
pub use crate::records::{versioned, versioned_error, ReceiptQueryError};

pub struct Reader {
    receipts_root: PathBuf,
    trust_path: PathBuf,
}

impl Reader {
    pub fn new(receipts_root: impl Into<PathBuf>, trust_path: impl Into<PathBuf>) -> Self {
        Self {
            receipts_root: receipts_root.into(),
            trust_path: trust_path.into(),
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
