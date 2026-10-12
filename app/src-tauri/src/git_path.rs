//! Finds the git the app runs. A GUI app gets launchd's PATH, not the
//! shell's, so the app looks only at fixed paths, once at start.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CANDIDATES: [&str; 2] = ["/opt/homebrew/bin/git", "/usr/local/bin/git"];

/// A stub until the Command Line Tools are installed; running it opens the
/// install dialog, so it counts only when `xcode-select -p` succeeds.
const SYSTEM_GIT: &str = "/usr/bin/git";

pub fn resolve() -> Option<PathBuf> {
    find(
        &CANDIDATES.map(Path::new),
        Path::new(SYSTEM_GIT),
        command_line_tools_installed,
    )
}

fn find(
    candidates: &[&Path],
    system_git: &Path,
    command_line_tools: impl FnOnce() -> bool,
) -> Option<PathBuf> {
    if let Some(found) = candidates.iter().find(|path| is_executable(path)) {
        return Some(found.to_path_buf());
    }
    (is_executable(system_git) && command_line_tools()).then(|| system_git.to_path_buf())
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn command_line_tools_installed() -> bool {
    Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("shipslip-app-git-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str, mode: u32) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, "").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn never_asked() -> bool {
        panic!("xcode-select must not run when an earlier git exists");
    }

    #[test]
    fn first_executable_candidate_wins_without_asking_xcode_select() {
        let dir = Dir::new("first");
        let brew = dir.file("brew-git", 0o755);
        let local = dir.file("local-git", 0o755);
        let system = dir.file("system-git", 0o755);
        assert_eq!(find(&[&brew, &local], &system, never_asked), Some(brew));
    }

    #[test]
    fn skips_missing_non_executable_and_directory_candidates() {
        let dir = Dir::new("skips");
        let missing = dir.0.join("missing");
        let plain = dir.file("plain", 0o644);
        let folder = dir.0.join("folder");
        fs::create_dir(&folder).unwrap();
        let local = dir.file("local-git", 0o755);
        let system = dir.file("system-git", 0o755);
        assert_eq!(
            find(&[&missing, &plain, &folder, &local], &system, never_asked),
            Some(local)
        );
    }

    #[test]
    fn system_git_needs_the_command_line_tools() {
        let dir = Dir::new("system");
        let missing = dir.0.join("missing");
        let system = dir.file("system-git", 0o755);
        assert_eq!(find(&[&missing], &system, || true), Some(system.clone()));
        assert_eq!(find(&[&missing], &system, || false), None);
    }

    #[test]
    fn no_git_anywhere_is_none() {
        let dir = Dir::new("none");
        let missing = dir.0.join("missing");
        assert_eq!(find(&[&missing], &dir.0.join("system-git"), || true), None);
    }
}
