//! A Cloud deployment's receipt: what Laravel Cloud reported about one
//! deployment it ran, kept beside run receipts in its own schema. Shipslip
//! did not start it, so nothing here is a run.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{io_error, private_dir, receipt_dir, ReceiptError};

/// Older releases see this version and report the receipt as written by a
/// newer Shipslip instead of misreading it as a run.
pub(crate) const CLOUD_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudKind {
    Cloud,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudReceipt {
    pub version: u32,
    pub kind: CloudKind,
    pub project: String,
    pub env: String,
    pub cloud_environment: String,
    pub deployment_id: String,
    /// The deployment status exactly as Laravel Cloud reported it.
    pub cloud_status: String,
    pub branch: String,
    pub commit_hash: String,
    pub commit_message: String,
    pub commit_author: Option<String>,
    pub failure_reason: Option<String>,
    /// Laravel Cloud's own clock, never this Mac's.
    pub reported: CloudReportedTimes,
    pub pulled_at_ms: u128,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudReportedTimes {
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

impl CloudReceipt {
    /// Laravel Cloud's start time in milliseconds, when it reported one this
    /// version can read.
    pub fn reported_started_at_ms(&self) -> Option<u128> {
        self.reported.started_at.as_deref().and_then(parse_time_ms)
    }
}

#[derive(Deserialize)]
struct KindProbe {
    #[serde(default)]
    kind: Option<String>,
}

pub(crate) fn is_cloud(bytes: &[u8]) -> bool {
    serde_json::from_slice::<KindProbe>(bytes)
        .is_ok_and(|probe| probe.kind.as_deref() == Some("cloud"))
}

pub fn read_cloud(path: &Path) -> Result<CloudReceipt, ReceiptError> {
    let bytes = fs::read(path).map_err(|source| io_error(path, source))?;
    let receipt: CloudReceipt =
        serde_json::from_slice(&bytes).map_err(|source| ReceiptError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    if receipt.version != CLOUD_VERSION {
        return Err(ReceiptError::Version(receipt.version));
    }
    if !is_plain_id(&receipt.deployment_id) {
        return Err(ReceiptError::Invalid("invalid Cloud deployment ID".into()));
    }
    Ok(receipt)
}

/// The held receipt for one Cloud deployment, if any.
pub fn find_cloud(
    root: &Path,
    project: &str,
    env: &str,
    deployment_id: &str,
) -> Result<Option<PathBuf>, ReceiptError> {
    let dir = receipt_dir(root, project, env);
    let suffix = format!("-{deployment_id}.json");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error(&dir, source)),
    };
    for entry in entries {
        let path = entry.map_err(|source| io_error(&dir, source))?.path();
        let named = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(&suffix));
        if named && read_cloud(&path).is_ok_and(|held| held.deployment_id == deployment_id) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Writes the receipt durably as `<start ms>-<deployment ID>.json`, so it
/// sorts with run receipts by Laravel Cloud's start time.
pub fn write_cloud(root: &Path, receipt: &CloudReceipt) -> Result<PathBuf, ReceiptError> {
    if !is_plain_id(&receipt.deployment_id) {
        return Err(ReceiptError::Invalid("invalid Cloud deployment ID".into()));
    }
    let dir = receipt_dir(root, &receipt.project, &receipt.env);
    fs::create_dir_all(&dir).map_err(|source| io_error(&dir, source))?;
    private_dir(&dir)?;
    let sort_ms = receipt
        .reported_started_at_ms()
        .unwrap_or(receipt.pulled_at_ms);
    let path = dir.join(format!("{sort_ms}-{}.json", receipt.deployment_id));
    let mut bytes = serde_json::to_vec_pretty(receipt)?;
    bytes.push(b'\n');
    crate::private_file::write_atomic(&path, &bytes).map_err(|source| io_error(&path, source))?;
    Ok(path)
}

/// Laravel Cloud IDs become file names and URL path segments.
pub(crate) fn is_plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// `2026-11-16T09:30:00Z`, with optional fraction and a `Z` or `±hh:mm`
/// offset, as milliseconds since the epoch.
pub(crate) fn parse_time_ms(text: &str) -> Option<u128> {
    let bytes = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let (hour, minute, second) = (digits(11..13)?, digits(14..16)?, digits(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &text[19..];
    let mut millis = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let end = fraction
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(fraction.len());
        if end == 0 {
            return None;
        }
        let padded = format!("{:0<3}", &fraction[..end.min(3)]);
        millis = padded.parse::<i64>().ok()?;
        rest = &fraction[end..];
    }
    let offset_minutes = match rest {
        "Z" | "z" => 0,
        _ if rest.len() == 6 && rest.as_bytes()[3] == b':' => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hours: i64 = rest[1..3].parse().ok()?;
            let minutes: i64 = rest[4..6].parse().ok()?;
            sign * (hours * 60 + minutes)
        }
        _ => return None,
    };
    // Civil date to days since 1970-01-01 (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_minutes * 60;
    let total = seconds.checked_mul(1000)?.checked_add(millis)?;
    u128::try_from(total).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipt::format_utc;

    #[test]
    fn cloud_times_parse_with_fractions_and_offsets() {
        let ms = parse_time_ms("2026-11-16T09:30:05.123456Z").unwrap();
        assert_eq!(ms % 1000, 123);
        assert_eq!(format_utc(ms), "2026-11-16 09:30:05Z");
        assert_eq!(
            parse_time_ms("2026-11-16T11:30:05+02:00"),
            parse_time_ms("2026-11-16T09:30:05Z")
        );
        assert_eq!(
            format_utc(parse_time_ms("2024-02-29T00:00:00Z").unwrap()),
            "2024-02-29 00:00:00Z"
        );
        for bad in [
            "",
            "yesterday",
            "2026-13-01T00:00:00Z",
            "2026-11-16T09:30:05",
            "2026-11-16T09:30:05.Z",
        ] {
            assert_eq!(parse_time_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_receipt_with_an_unsafe_id_is_neither_written_nor_read() {
        let root = crate::receipt::tests::TempRoot::new();
        let mut receipt = CloudReceipt {
            version: CLOUD_VERSION,
            kind: CloudKind::Cloud,
            project: "app".into(),
            env: "production".into(),
            cloud_environment: "env-1".into(),
            deployment_id: "../escape".into(),
            cloud_status: "deployment.succeeded".into(),
            branch: "main".into(),
            commit_hash: "abc".into(),
            commit_message: "m".into(),
            commit_author: None,
            failure_reason: None,
            reported: CloudReportedTimes::default(),
            pulled_at_ms: 7,
        };
        assert!(matches!(
            write_cloud(&root.0, &receipt),
            Err(ReceiptError::Invalid(_))
        ));
        receipt.deployment_id = "depl-1".into();
        let path = write_cloud(&root.0, &receipt).unwrap();
        assert!(path.ends_with("app/production/7-depl-1.json"));
        assert_eq!(read_cloud(&path).unwrap(), receipt);
        let mut forged = serde_json::to_value(&receipt).unwrap();
        forged["deployment_id"] = "a/b".into();
        std::fs::write(&path, forged.to_string()).unwrap();
        assert!(matches!(read_cloud(&path), Err(ReceiptError::Invalid(_))));
        assert_eq!(
            find_cloud(&root.0, "app", "production", "depl-1").unwrap(),
            None
        );
    }

    #[test]
    fn deployment_ids_that_could_change_a_path_are_refused() {
        assert!(is_plain_id("depl-9f2c41d0_A"));
        for bad in ["", "-x", "../x", "a/b", "a b", "a.json"] {
            assert!(!is_plain_id(bad), "{bad}");
        }
    }
}
