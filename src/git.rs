use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};
use regex::Regex;

use crate::{
    cli::ChangeArgs,
    config::Config,
    model::{ChangeSet, ChangeStatus, ChangedFile, LineRange, RemovedLine},
    repo::Repository,
};

pub fn changes(repo: &Repository, config: &Config, args: &ChangeArgs) -> Result<ChangeSet> {
    let mut files: BTreeMap<String, ChangedFile> = BTreeMap::new();
    let mut removed_lines = Vec::new();
    for path in &args.files {
        let path = normalize_explicit_path(repo, path)?;
        files.insert(
            path.clone(),
            ChangedFile {
                path,
                old_path: None,
                status: ChangeStatus::Modified,
                ranges: Vec::new(),
                whole_file: true,
            },
        );
    }

    let use_git = args.files.is_empty()
        || args.diff.is_some()
        || args.staged
        || args.worktree
        || args.untracked;
    let mut description_parts = Vec::new();
    if use_git {
        ensure_git_repository(repo)?;
        let revision = args
            .diff
            .clone()
            .unwrap_or_else(|| config.git.default_diff.clone());
        let (patch, description) = if args.staged {
            (
                run_diff(repo, None, true, config.git.detect_renames)?,
                "staged changes".to_string(),
            )
        } else if args.worktree {
            (
                run_diff(repo, None, false, config.git.detect_renames)?,
                "worktree changes".to_string(),
            )
        } else if has_head(repo) {
            (
                run_diff(repo, Some(&revision), false, config.git.detect_renames)?,
                format!("diff {revision}"),
            )
        } else {
            (
                run_diff(repo, None, true, config.git.detect_renames)?,
                "initial repository changes".to_string(),
            )
        };
        parse_patch(&patch, &mut files, &mut removed_lines)?;
        description_parts.push(description);

        let include_untracked = if args.no_untracked {
            false
        } else {
            args.untracked || (config.git.include_untracked && !args.staged)
        };
        if include_untracked {
            for path in untracked(repo)? {
                files.entry(path.clone()).or_insert(ChangedFile {
                    path,
                    old_path: None,
                    status: ChangeStatus::Untracked,
                    ranges: Vec::new(),
                    whole_file: true,
                });
            }
            description_parts.push("untracked files".into());
        }
    } else {
        description_parts.push("explicit files".into());
    }

    let mut change_set = ChangeSet {
        description: description_parts.join(" and "),
        fingerprint: String::new(),
        files: files.into_values().collect(),
        removed_lines,
    };
    change_set.fingerprint = fingerprint(&change_set)?;
    Ok(change_set)
}

fn ensure_git_repository(repo: &Repository) -> Result<()> {
    let output = git(repo, ["rev-parse", "--is-inside-work-tree"])?;
    if !output.status.success() || output.stdout != b"true\n" {
        bail!(
            "{} is not a Git work tree; use one or more --file options for explicit analysis",
            repo.root.display()
        );
    }
    Ok(())
}

fn has_head(repo: &Repository) -> bool {
    git(repo, ["rev-parse", "--verify", "HEAD"]).is_ok_and(|output| output.status.success())
}

fn run_diff(
    repo: &Repository,
    revision: Option<&str>,
    staged: bool,
    detect_renames: bool,
) -> Result<String> {
    let mut command = Command::new("git");
    command
        .current_dir(&repo.root)
        .args(["-c", "core.quotePath=false", "diff"])
        .args([
            "--unified=0",
            "--no-color",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ]);
    if detect_renames {
        command.args(["--find-renames", "--find-copies"]);
    } else {
        command.arg("--no-renames");
    }
    if staged {
        command.arg("--cached");
    }
    if let Some(revision) = revision {
        command.arg(revision);
    }
    command.arg("--");
    let output = command.output().context("failed to execute git diff")?;
    if !output.status.success() {
        bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("git diff output was not UTF-8")
}

fn parse_patch(
    patch: &str,
    files: &mut BTreeMap<String, ChangedFile>,
    removed_lines: &mut Vec<RemovedLine>,
) -> Result<()> {
    let hunk = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")?;
    let mut old_path: Option<String> = None;
    let mut new_path: Option<String> = None;
    let mut rename_from: Option<String> = None;
    let mut rename_to: Option<String> = None;
    let mut copy_from: Option<String> = None;
    let mut copy_to: Option<String> = None;
    let mut old_line = 0_u32;
    let mut new_line = 0_u32;
    let mut in_hunk = false;

    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            finalize_file(
                files,
                &old_path,
                &new_path,
                &rename_from,
                &rename_to,
                &copy_from,
                &copy_to,
            );
            old_path = None;
            new_path = None;
            rename_from = None;
            rename_to = None;
            copy_from = None;
            copy_to = None;
            in_hunk = false;
            continue;
        }
        if let Some(path) = line.strip_prefix("rename from ") {
            rename_from = Some(unquote_path(path));
            continue;
        }
        if let Some(path) = line.strip_prefix("rename to ") {
            rename_to = Some(unquote_path(path));
            continue;
        }
        if let Some(path) = line.strip_prefix("copy from ") {
            copy_from = Some(unquote_path(path));
            continue;
        }
        if let Some(path) = line.strip_prefix("copy to ") {
            copy_to = Some(unquote_path(path));
            continue;
        }
        if let Some(path) = line.strip_prefix("--- ") {
            old_path = patch_path(path, "a/");
            continue;
        }
        if let Some(path) = line.strip_prefix("+++ ") {
            new_path = patch_path(path, "b/");
            if let Some(path) = new_path.as_ref().or(old_path.as_ref()) {
                files.entry(path.clone()).or_insert_with(|| ChangedFile {
                    path: path.clone(),
                    old_path: old_path
                        .clone()
                        .filter(|old| Some(old) != new_path.as_ref()),
                    status: if old_path.is_none() {
                        ChangeStatus::Added
                    } else if new_path.is_none() {
                        ChangeStatus::Deleted
                    } else {
                        ChangeStatus::Modified
                    },
                    ranges: Vec::new(),
                    whole_file: false,
                });
            }
            continue;
        }
        if let Some(captures) = hunk.captures(line) {
            old_line = captures[1].parse()?;
            new_line = captures[3].parse()?;
            let new_count = captures
                .get(4)
                .map_or(Ok(1), |value| value.as_str().parse::<u32>())?;
            if let Some(path) = new_path.as_ref().or(old_path.as_ref()) {
                let changed = files.entry(path.clone()).or_insert_with(|| ChangedFile {
                    path: path.clone(),
                    old_path: old_path.clone(),
                    status: ChangeStatus::Modified,
                    ranges: Vec::new(),
                    whole_file: false,
                });
                let start = new_line.max(1);
                let end = if new_count == 0 {
                    start
                } else {
                    start.saturating_add(new_count - 1)
                };
                merge_range(&mut changed.ranges, LineRange { start, end });
            }
            in_hunk = true;
            continue;
        }
        if in_hunk {
            if let Some(text) = line.strip_prefix('-') {
                if let Some(path) = old_path.as_ref().or(new_path.as_ref()) {
                    removed_lines.push(RemovedLine {
                        path: path.clone(),
                        line: old_line,
                        text: text.into(),
                    });
                }
                old_line = old_line.saturating_add(1);
            } else if line.starts_with('+') {
                new_line = new_line.saturating_add(1);
            } else if line.starts_with(' ') {
                old_line = old_line.saturating_add(1);
                new_line = new_line.saturating_add(1);
            }
        }
    }
    finalize_file(
        files,
        &old_path,
        &new_path,
        &rename_from,
        &rename_to,
        &copy_from,
        &copy_to,
    );
    for changed in files.values_mut() {
        if changed.ranges.is_empty() {
            changed.whole_file = true;
        }
    }
    Ok(())
}

fn finalize_file(
    files: &mut BTreeMap<String, ChangedFile>,
    old_path: &Option<String>,
    new_path: &Option<String>,
    rename_from: &Option<String>,
    rename_to: &Option<String>,
    copy_from: &Option<String>,
    copy_to: &Option<String>,
) {
    let old = rename_from
        .as_ref()
        .or(copy_from.as_ref())
        .or(old_path.as_ref());
    let new = rename_to
        .as_ref()
        .or(copy_to.as_ref())
        .or(new_path.as_ref());
    let Some(path) = new.or(old) else {
        return;
    };
    let changed = files.entry(path.clone()).or_insert_with(|| ChangedFile {
        path: path.clone(),
        old_path: None,
        status: ChangeStatus::Modified,
        ranges: Vec::new(),
        whole_file: true,
    });
    if rename_from.is_some() && rename_to.is_some() {
        changed.status = ChangeStatus::Renamed;
        changed.old_path = rename_from.clone();
    } else if copy_from.is_some() && copy_to.is_some() {
        changed.status = ChangeStatus::Copied;
        changed.old_path = copy_from.clone();
    } else if new_path.is_none() {
        changed.status = ChangeStatus::Deleted;
        changed.old_path = old_path.clone();
    } else if old_path.is_none() {
        changed.status = ChangeStatus::Added;
    }
}

fn patch_path(value: &str, prefix: &str) -> Option<String> {
    if value == "/dev/null" {
        return None;
    }
    let value = unquote_path(value);
    Some(value.strip_prefix(prefix).unwrap_or(&value).to_string())
}

fn unquote_path(value: &str) -> String {
    let value = value.trim();
    if !(value.starts_with('"') && value.ends_with('"')) {
        return value.into();
    }
    let mut output = String::new();
    let mut chars = value[1..value.len() - 1].chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('t') => output.push('\t'),
            Some('r') => output.push('\r'),
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some(other) => output.push(other),
            None => output.push('\\'),
        }
    }
    output
}

fn merge_range(ranges: &mut Vec<LineRange>, next: LineRange) {
    if let Some(last) = ranges.last_mut() {
        if next.start <= last.end.saturating_add(1) {
            last.end = last.end.max(next.end);
            return;
        }
    }
    ranges.push(next);
}

fn untracked(repo: &Repository) -> Result<Vec<String>> {
    let output = git(repo, ["ls-files", "--others", "--exclude-standard", "-z"])?;
    if !output.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).replace('\\', "/"))
        .collect())
}

fn git<const N: usize>(repo: &Repository, args: [&str; N]) -> Result<Output> {
    Command::new("git")
        .current_dir(&repo.root)
        .args(args)
        .output()
        .context("failed to execute git")
}

fn normalize_explicit_path(repo: &Repository, value: &str) -> Result<String> {
    let path = Path::new(value);
    if path.is_absolute() {
        return path
            .strip_prefix(&repo.root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .with_context(|| format!("{} is outside {}", path.display(), repo.root.display()));
    }
    let normalized = value.trim_start_matches("./").replace('\\', "/");
    if normalized.split('/').any(|part| part == "..") {
        bail!("explicit path may not escape the repository: {value}");
    }
    Ok(normalized)
}

fn fingerprint(change_set: &ChangeSet) -> Result<String> {
    let mut copy = change_set.clone();
    copy.fingerprint.clear();
    Ok(blake3::hash(&serde_json::to_vec(&copy)?)
        .to_hex()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges_deletions_and_renames() {
        let patch = r#"diff --git a/old.rs b/new.rs
similarity index 90%
rename from old.rs
rename to new.rs
--- a/old.rs
+++ b/new.rs
@@ -2,2 +2,3 @@
-old
+new
+extra
 context
"#;
        let mut files = BTreeMap::new();
        let mut removed = Vec::new();
        parse_patch(patch, &mut files, &mut removed).unwrap();
        let changed = files.get("new.rs").unwrap();
        assert_eq!(changed.status, ChangeStatus::Renamed);
        assert_eq!(changed.old_path.as_deref(), Some("old.rs"));
        assert_eq!(removed[0].text, "old");
    }
}
