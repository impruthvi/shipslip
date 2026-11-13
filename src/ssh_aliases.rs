//! The `Host` aliases in the user's ssh config, for picking one in the app.
//! Shipslip only reads ssh files (ADR 0003): it lists aliases, asks
//! `ssh -G` what one resolves to, and builds a `Host` block for the user to
//! paste themselves. Listing reads `Host` and `Include` lines only to
//! offer names; what an alias means is always `ssh -G`'s answer.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::is_ssh_alias;

/// ssh stops following `Include` at the same depth.
const INCLUDE_DEPTH: usize = 16;

pub fn ssh_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".ssh"))
}

pub fn user_config() -> Option<PathBuf> {
    ssh_dir().map(|dir| dir.join("config"))
}

/// The `Host` names in `config` and the files it includes, in file order,
/// each once. Patterns (`*`, `?`, `!`) are left out: they are not an alias
/// anyone connects to. A missing config lists nothing; an included file
/// that can't be read is skipped, as ssh does.
pub fn list(config: &Path) -> io::Result<Vec<String>> {
    let text = match fs::read_to_string(config) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let base = config.parent().unwrap_or(Path::new("/"));
    let mut aliases = Vec::new();
    collect(&text, base, 0, &mut aliases);
    Ok(aliases)
}

fn collect(text: &str, base: &Path, depth: usize, aliases: &mut Vec<String>) {
    for line in text.lines() {
        let Some((keyword, args)) = directive(line) else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("host") {
            for name in args {
                if is_ssh_alias(&name) && !aliases.contains(&name) {
                    aliases.push(name);
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") && depth < INCLUDE_DEPTH {
            for pattern in args {
                for file in expand(&pattern, base) {
                    if let Ok(text) = fs::read_to_string(&file) {
                        collect(&text, base, depth + 1, aliases);
                    }
                }
            }
        }
    }
}

/// A config line's keyword and arguments: `Keyword args` or `Keyword=args`,
/// double quotes grouping, `#` starting a comment.
fn directive(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    let split = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let keyword = &line[..split];
    let rest = line[split..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    let mut args = Vec::new();
    let mut chars = rest.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        match chars.peek() {
            None | Some('#') => break,
            Some('"') => {
                chars.next();
                args.push(chars.by_ref().take_while(|&c| c != '"').collect());
            }
            Some(_) => {
                let mut arg = String::new();
                while let Some(&c) = chars.peek().filter(|c| !c.is_whitespace()) {
                    arg.push(c);
                    chars.next();
                }
                args.push(arg);
            }
        }
    }
    (!keyword.starts_with('#')).then(|| (keyword.to_string(), args))
}

/// The files an `Include` argument names: `~` is the home folder, a
/// relative path is inside the config's folder, and the last part may hold
/// `*` and `?`. Sorted, as ssh reads them.
fn expand(pattern: &str, base: &Path) -> Vec<PathBuf> {
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => return Vec::new(),
        },
        None => base.join(pattern),
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_str().is_some_and(|n| glob(name, n)))
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    files
}

fn glob(pattern: &str, name: &str) -> bool {
    fn at(p: &[char], n: &[char]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some(('*', rest)) => (0..=n.len()).any(|skip| at(rest, &n[skip..])),
            Some((&c, rest)) => n
                .split_first()
                .is_some_and(|(&first, tail)| (c == '?' || c == first) && at(rest, tail)),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    at(&p, &n)
}

/// What ssh connects to for an alias, as `ssh -G` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub host_name: String,
    pub user: String,
    pub port: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("not an ssh alias")]
    NotAnAlias,
    #[error("could not run ssh: {0}")]
    Spawn(io::Error),
    #[error("ssh -G failed: {0}")]
    Failed(String),
    #[error("ssh -G printed no {0}")]
    Missing(&'static str),
}

/// Asks `ssh -G` what `alias` resolves to, so ssh applies `Include`,
/// `Match` and `Host *` itself. `config` replaces `~/.ssh/config` (`-F`).
/// Nothing connects. An alias no `Host` names still resolves, to itself.
pub fn resolve(alias: &str, config: Option<&Path>) -> Result<Resolved, ResolveError> {
    if !is_ssh_alias(alias) {
        return Err(ResolveError::NotAnAlias);
    }
    let mut command = Command::new("ssh");
    if let Some(config) = config {
        command.arg("-F").arg(config);
    }
    let output = command
        .args(["-G", "--", alias])
        .output()
        .map_err(ResolveError::Spawn)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ResolveError::Failed(stderr.trim().to_string()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value = |key: &'static str| {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
            .map(str::to_string)
            .ok_or(ResolveError::Missing(key))
    };
    Ok(Resolved {
        host_name: value("hostname")?,
        user: value("user")?,
        port: value("port")?
            .parse()
            .map_err(|_| ResolveError::Missing("port"))?,
    })
}

/// The fields "None of these fit" asks for, as typed.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDraft {
    pub alias: String,
    pub host_name: String,
    /// Empty: ssh's default user.
    pub user: String,
    /// One of [`identity_files`], or empty for ssh's default keys.
    pub identity_file: String,
    /// Empty means 22.
    pub port: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HostField {
    Alias,
    HostName,
    User,
    IdentityFile,
    Port,
}

impl HostDraft {
    /// The `Host` block for `~/.ssh/config`, or each field that can't go in
    /// one. A value never adds a line or a second word, so the block holds
    /// only these settings. Port 22 is left out.
    pub fn block(&self) -> Result<String, Vec<(HostField, &'static str)>> {
        let problems = self.problems();
        if !problems.is_empty() {
            return Err(problems);
        }
        let mut block = format!(
            "Host {}\n  HostName {}\n",
            self.alias.trim(),
            self.host_name.trim()
        );
        if !self.user.trim().is_empty() {
            block.push_str(&format!("  User {}\n", self.user.trim()));
        }
        if !self.identity_file.trim().is_empty() {
            block.push_str(&format!("  IdentityFile {}\n", self.identity_file.trim()));
        }
        let port = self.port_number().unwrap_or(22);
        if port != 22 {
            block.push_str(&format!("  Port {port}\n"));
        }
        Ok(block)
    }

    /// The draft's alias now resolves to its host name, port and (when
    /// given) user.
    pub fn matches(&self, resolved: &Resolved) -> bool {
        let user = self.user.trim();
        resolved
            .host_name
            .eq_ignore_ascii_case(self.host_name.trim())
            && (user.is_empty() || resolved.user == user)
            && Some(resolved.port) == self.port_number()
    }

    fn port_number(&self) -> Option<u16> {
        let port = self.port.trim();
        if port.is_empty() {
            return Some(22);
        }
        port.parse().ok().filter(|&port| port != 0)
    }

    fn problems(&self) -> Vec<(HostField, &'static str)> {
        let word = |value: &str, extra: &[char]| {
            value.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') || extra.contains(&c)
            }) && !value.starts_with('-')
        };
        let mut problems = Vec::new();
        if !is_ssh_alias(self.alias.trim()) {
            problems.push((
                HostField::Alias,
                "Use only letters, digits, `.`, `-` and `_`.",
            ));
        }
        let host_name = self.host_name.trim();
        if host_name.is_empty() {
            problems.push((
                HostField::HostName,
                "Enter the server's host name or IP address.",
            ));
        } else if !word(host_name, &[':']) {
            problems.push((
                HostField::HostName,
                "Use a host name or IP address, such as 203.0.113.7.",
            ));
        }
        if !word(self.user.trim(), &[]) {
            problems.push((
                HostField::User,
                "Use only letters, digits, `.`, `-` and `_`.",
            ));
        }
        let identity = self.identity_file.trim();
        if !identity.is_empty()
            && !(identity.starts_with("~/.ssh/id_") && word(&identity["~/.ssh/".len()..], &[]))
        {
            problems.push((HostField::IdentityFile, "Pick a key from ~/.ssh."));
        }
        if self.port_number().is_none() {
            problems.push((HostField::Port, "Use a port from 1 to 65535."));
        }
        problems
    }
}

/// The private keys in `ssh_dir` named `id_*`, as `~/.ssh/<name>`, sorted.
pub fn identity_files(ssh_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(ssh_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("id_") && !name.ends_with(".pub"))
        .map(|name| format!("~/.ssh/{name}"))
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests;
