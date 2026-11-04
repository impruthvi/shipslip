use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub type ReceiptStamp = BTreeMap<PathBuf, SystemTime>;

/// Journals are replaced atomically, changing the environment directory's mtime.
/// Only stat the root, project and environment directories, never receipt contents.
pub fn read(root: &Path) -> io::Result<ReceiptStamp> {
    let mut stamp = BTreeMap::new();
    let mut pending = vec![(root.to_path_buf(), 0)];
    while let Some((path, depth)) = pending.pop() {
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        stamp.insert(path.clone(), metadata.modified()?);
        if depth < 2 {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    pending.push((entry.path(), depth + 1));
                }
            }
        }
    }
    Ok(stamp)
}
