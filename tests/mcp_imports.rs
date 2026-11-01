//! The MCP implementation may only reach the read-only library facade.

use std::fs;
use std::path::Path;

fn forbidden_paths(source: &str) -> Vec<String> {
    let tokens = tokens(source);
    tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| {
            let next = tokens.get(index + 1).map(String::as_str);
            let segment = tokens.get(index + 2).map(String::as_str);
            let previous = tokens.get(index.wrapping_sub(1)).map(String::as_str);
            let forbidden = match token.as_str() {
                "shipslip" => {
                    next == Some("::") && segment != Some("read_only")
                        || matches!(next, Some("as"))
                        || next != Some("::")
                            && tokens
                                .get(index.wrapping_sub(1))
                                .is_some_and(|prev| prev == "use" || prev == "crate")
                }
                "super" => next == Some("::"),
                "crate" => next == Some("::") && segment != Some("mcp") || next == Some("as"),
                "include" => next == Some("!"),
                "path" => next == Some("=") && matches!(previous, Some("[" | ",")),
                // Reject identifiers too, so grouped imports and aliases cannot hide spawning.
                "process" | "Command" => true,
                _ => false,
            };
            forbidden.then(|| tokens[index..tokens.len().min(index + 3)].join(" "))
        })
        .collect()
}

// Lex strings and comments so neither can hide a path or create a false match.
fn tokens(mut source: &str) -> Vec<String> {
    let mut result = Vec::new();
    while !source.is_empty() {
        if source.starts_with("//") {
            source = source.find('\n').map_or("", |end| &source[end..]);
        } else if let Some(mut rest) = source.strip_prefix("/*") {
            let mut depth = 1;
            while !rest.is_empty() && depth > 0 {
                if let Some(after) = rest.strip_prefix("/*") {
                    depth += 1;
                    rest = after;
                } else if let Some(after) = rest.strip_prefix("*/") {
                    depth -= 1;
                    rest = after;
                } else {
                    rest = &rest[rest.chars().next().unwrap().len_utf8()..];
                }
            }
            source = rest;
        } else if let Some(len) = character_literal_len(source) {
            source = &source[len..];
        } else if let Some(rest) = source
            .strip_prefix("r#")
            .filter(|rest| rest.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_'))
        {
            source = rest;
        } else if source.starts_with('r') && source[1..].trim_start_matches('#').starts_with('"') {
            let hashes = source[1..].len() - source[1..].trim_start_matches('#').len();
            let rest = &source[hashes + 2..];
            let end = format!("\"{}", "#".repeat(hashes));
            source = rest
                .find(&end)
                .map_or("", |offset| &rest[offset + end.len()..]);
        } else if let Some(mut rest) = source.strip_prefix('"') {
            while !rest.is_empty() {
                let ch = rest.chars().next().unwrap();
                rest = &rest[ch.len_utf8()..];
                if ch == '"' {
                    break;
                }
                if ch == '\\' && !rest.is_empty() {
                    rest = &rest[rest.chars().next().unwrap().len_utf8()..];
                }
            }
            source = rest;
        } else if source.starts_with("::") {
            result.push("::".into());
            source = &source[2..];
        } else {
            let ch = source.chars().next().unwrap();
            if ch.is_ascii_alphabetic() || ch == '_' {
                let end = source
                    .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                    .unwrap_or(source.len());
                result.push(source[..end].into());
                source = &source[end..];
            } else {
                if !ch.is_whitespace() && ch != '#' {
                    result.push(ch.to_string());
                }
                source = &source[ch.len_utf8()..];
            }
        }
    }
    result
}

fn character_literal_len(source: &str) -> Option<usize> {
    let rest = source.strip_prefix('\'')?;
    let end = if rest.starts_with('\\') {
        match rest.as_bytes().get(1)? {
            b'u' => rest.find('}')? + 1,
            b'x' => 4,
            _ => 2,
        }
    } else {
        rest.chars().next()?.len_utf8()
    };
    rest.get(end..)?.starts_with('\'').then_some(end + 2)
}

fn check_sources(path: &Path, checked: &mut usize) {
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            check_sources(&entry.unwrap().path(), checked);
        }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
        let forbidden = forbidden_paths(&fs::read_to_string(path).unwrap());
        assert!(forbidden.is_empty(), "{}: {forbidden:?}", path.display());
        *checked += 1;
    }
}

#[test]
fn mcp_sources_only_import_the_read_only_facade_or_mcp_modules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin/slip");
    let mut checked = 0;
    check_sources(&root.join("mcp.rs"), &mut checked);
    if root.join("mcp").exists() {
        check_sources(&root.join("mcp"), &mut checked);
    }
    assert!(checked > 0);
}

#[test]
fn allow_list_rejects_forbidden_paths_including_new_library_apis() {
    for source in [
        "use shipslip::execute;",
        "shipslip::a_future_mutation();",
        "use shipslip::{read_only::Reader, execute};",
        "use shipslip::read_only_extra::Reader;",
        "use ::shipslip /* comment */ ::execute as run;",
        "use shipslip /* outer /* inner */ comment */ ::config::*;",
        "use shipslip as api;",
        "extern crate shipslip as api;",
        "use super::execute;",
        "super :: super :: execute();",
        "use crate::execute;",
        "use crate::{mcp, execute};",
        "use crate::mcp_extra::execute;",
        "use crate as cli;",
        "use r#shipslip::execute;",
        "let quote = '\"'; shipslip::execute();",
        "let quote = '\\u{22}'; shipslip::execute();",
        "include!(\"../deploy.rs\");",
        "std::include /* comment */ ! {\"../deploy.rs\"}",
        "#[path = \"../deploy.rs\"] mod deploy;",
        "#[r#path = \"../deploy.rs\"] mod deploy;",
        "#[cfg_attr(unix, path = \"../deploy.rs\")] mod deploy;",
        "use std::process;",
        "use tokio::process as runner;",
        "use std::{process::Command as Runner};",
        "use tokio::{process::{self as runner}};",
        "std::process::Command::new(\"ssh\").spawn();",
        "tokio::process::Command::new(\"slip\").spawn();",
        "Command::new(\"ssh\").spawn();",
        "use std as system; system::process::Command::new(\"ssh\");",
    ] {
        assert!(!forbidden_paths(source).is_empty(), "accepted {source}");
    }
    for source in [
        "use shipslip::read_only::{Reader, versioned};",
        "use crate::mcp::tools;",
        "// use shipslip::execute;\nuse shipslip::read_only::Reader;",
        "let text = \"shipslip::execute()\";",
        "let text = r###\"shipslip::execute()\"###;",
        "// include!(\"../deploy.rs\"); Command::new(\"ssh\");",
        "/* #[path = \"../deploy.rs\"] mod deploy; use std::process; */",
        "let text = r###\"include!(\"../deploy.rs\"); #[path = \"deploy.rs\"] mod deploy; Command::new(\"ssh\");\"###;",
    ] {
        assert!(forbidden_paths(source).is_empty(), "rejected {source}");
    }
}
