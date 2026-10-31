//! Read results built from the receipts on this Mac: what a front-end shows
//! or prints as JSON. Nothing here touches a server, a deploy lock or a
//! journal.

use serde::{Serialize, Serializer};

use crate::logs::{escape, escape_field};

mod next_steps;
mod outcome;
mod receipt;
mod steps;

pub use crate::receipt::markdown::render as markdown;
pub use next_steps::{copy_is_safe, failed_step_options, NextStep, NextSteps, Part};
pub use outcome::{describe_plan, Badge, Flag, Outcome, OutcomeKind};
pub use receipt::{
    receipt, receipt_at, ErrorGroup, ErrorVariant, ReceiptQueryError, ReceiptRecord, Smoke, Target,
    Watch,
};
pub use steps::{StepKind, StepRow, StepState};

/// The JSON schema number. Within one number fields are only added.
pub const SCHEMA: u32 = 1;

/// Text from a server or a receipt, held as it was recorded. It has no
/// `Display`, so printing it means choosing an escape:
///
/// ```compile_fail
/// let text = shipslip::records::Untrusted::new("\u{1b}[2J");
/// println!("{text}");
/// ```
///
/// ```
/// let text = shipslip::records::Untrusted::new("\u{1b}[2J");
/// assert_eq!(text.field(), "\\x1b[2J");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Untrusted(String);

impl Untrusted {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The recorded text, for checks and for renderers that escape it
    /// themselves.
    pub fn raw(&self) -> &str {
        &self.0
    }

    /// Control characters escaped; newlines and tabs kept.
    pub fn escaped(&self) -> String {
        escape(&self.0)
    }

    /// Control characters, newlines and tabs escaped, for one line.
    pub fn field(&self) -> String {
        escape_field(&self.0)
    }
}

impl From<&str> for Untrusted {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for Untrusted {
    fn from(text: String) -> Self {
        Self(text)
    }
}

impl Serialize for Untrusted {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// A result as JSON: `{"schema": 1, ...}`.
#[derive(Debug, Serialize)]
pub struct Versioned<T> {
    pub schema: u32,
    #[serde(flatten)]
    pub body: T,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody<E> {
    pub error: E,
}

pub fn versioned<T: Serialize>(body: T) -> Versioned<T> {
    Versioned {
        schema: SCHEMA,
        body,
    }
}

/// An error as JSON: `{"schema": 1, "error": {"kind": ...}}`.
pub fn versioned_error<E: Serialize>(error: E) -> Versioned<ErrorBody<E>> {
    versioned(ErrorBody { error })
}

#[cfg(test)]
mod tests;
