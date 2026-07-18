use std::{fs, io::Write, path::PathBuf};

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Serialize;

use crate::{annotations, config::Config, index, model::Severity, repo::Repository};

#[derive(Debug, Serialize)]
pub struct DynamicLocationMigration {
    pub files_scanned: usize,
    pub files_changed: usize,
    pub annotations_migrated: usize,
    pub changed_paths: Vec<String>,
}

struct PendingFile {
    path: PathBuf,
    relative: String,
    bytes: Vec<u8>,
    annotations: usize,
}

pub fn dynamic_locations(
    repo: &Repository,
    config: &Config,
    check_only: bool,
) -> Result<DynamicLocationMigration> {
    let paths = index::source_paths(repo, config)?;
    let mut pending = Vec::new();
    for path in &paths {
        let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let (migrated, annotations) = migrate_text(text, config)?;
        if annotations == 0 {
            continue;
        }
        validate_equivalence(repo, config, path, text, &migrated)?;
        pending.push(PendingFile {
            path: path.clone(),
            relative: repo.relative(path),
            bytes: migrated.into_bytes(),
            annotations,
        });
    }
    pending.sort_by(|left, right| left.relative.cmp(&right.relative));
    let report = DynamicLocationMigration {
        files_scanned: paths.len(),
        files_changed: pending.len(),
        annotations_migrated: pending.iter().map(|file| file.annotations).sum(),
        changed_paths: pending.iter().map(|file| file.relative.clone()).collect(),
    };
    if !check_only {
        for file in pending {
            atomic_write(&file.path, &file.bytes)?;
        }
    }
    Ok(report)
}

fn migrate_text(text: &str, config: &Config) -> Result<(String, usize)> {
    let static_range = Regex::new(
        r#"[ \t]+(?:lines|line_start|line_end)=(?:\"(?:\\.|[^\"])*\"|'(?:\\.|[^'])*'|[^\s]+)"#,
    )?;
    let explicit_line = Regex::new(r#"[ \t]+line=(?:\"(?:\\.|[^\"])*\"|'(?:\\.|[^'])*'|[^\s]+)"#)?;
    let range_watch = Regex::new(r#"[ \t]+watch=(?:\"range\"|'range'|range)"#)?;
    let any_watch = Regex::new(r#"(?:^|[ \t])watch="#)?;
    let mut output = String::with_capacity(text.len());
    let mut migrated = 0;
    for segment in text.split_inclusive('\n') {
        let mut line = segment.to_string();
        let mut ranges = annotation_ranges(&line, config);
        for (start, end) in ranges.drain(..).rev() {
            let body = &line[start..end];
            let has_static_range = static_range.is_match(body) || range_watch.is_match(body);
            let has_explicit_line = explicit_line.is_match(body);
            if !has_static_range && !has_explicit_line {
                continue;
            }
            let mut replacement = static_range.replace_all(body, "").into_owned();
            replacement = range_watch.replace_all(&replacement, "").into_owned();
            if has_explicit_line {
                replacement = explicit_line.replace_all(&replacement, "").into_owned();
                if !has_static_range && !any_watch.is_match(&replacement) {
                    replacement = replacement.trim_end_matches([' ', '\t']).to_string();
                    replacement.push_str(" watch=line ");
                }
            }
            line.replace_range(start..end, &replacement);
            migrated += 1;
        }
        output.push_str(&line);
    }
    Ok((output, migrated))
}

fn annotation_ranges(line: &str, config: &Config) -> Vec<(usize, usize)> {
    let mut markers = Vec::new();
    for marker in &config.annotations.anchor_markers {
        let mut offset = 0;
        while let Some(relative) = line[offset..].find(marker) {
            let start = offset + relative;
            markers.push((start, start + marker.len(), true));
            offset = start + marker.len();
        }
    }
    for marker in config
        .annotations
        .strand_markers
        .iter()
        .chain(&config.annotations.relation_markers)
    {
        let mut offset = 0;
        while let Some(relative) = line[offset..].find(marker) {
            let start = offset + relative;
            markers.push((start, start + marker.len(), false));
            offset = start + marker.len();
        }
    }
    markers.sort_by_key(|marker| marker.0);
    let mut ranges = Vec::new();
    for (index, (_, body_start, anchor)) in markers.iter().copied().enumerate() {
        if !anchor {
            continue;
        }
        let body_end = markers.get(index + 1).map_or(line.len(), |marker| marker.0);
        ranges.push((body_start, body_end));
    }
    ranges
}

fn validate_equivalence(
    repo: &Repository,
    config: &Config,
    path: &std::path::Path,
    before: &str,
    after: &str,
) -> Result<()> {
    let relative = repo.relative(path);
    let old = annotations::scan_source(&relative, before, &config.annotations);
    let new = annotations::scan_source(&relative, after, &config.annotations);
    let old_ids: Vec<_> = old.anchors.iter().map(|item| item.0.id.as_str()).collect();
    let new_ids: Vec<_> = new.anchors.iter().map(|item| item.0.id.as_str()).collect();
    if old_ids != new_ids
        || old.memberships.len() != new.memberships.len()
        || old.relations.len() != new.relations.len()
    {
        bail!("dynamic-location migration changed graph semantics in {relative}");
    }
    let failures: Vec<_> = new
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect();
    if !failures.is_empty() {
        let messages = failures
            .iter()
            .map(|diagnostic| {
                format!(
                    "{}:{}: {}",
                    relative,
                    diagnostic.line.unwrap_or_default(),
                    diagnostic.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        bail!("dynamic-location migration could not resolve every anchor:\n{messages}");
    }
    Ok(())
}

fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("source path has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temporary file in {}", parent.display()))?;
    if let Ok(metadata) = fs::metadata(path) {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())?;
    }
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_authored_anchor_locations() {
        let config = Config::default();
        let source = "// @anchor api.run lines=10-20 @strand api role=handler\nfn run(value: u32 /* @anchor api.run.value watch=line */) {}\n";
        let (migrated, count) = migrate_text(source, &config).unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            migrated,
            "// @anchor api.run @strand api role=handler\nfn run(value: u32 /* @anchor api.run.value watch=line */) {}\n"
        );
    }

    #[test]
    fn preserves_line_semantics_without_static_coordinates() {
        let config = Config::default();
        let source =
            "// @anchor api.flag line=40 @strand api role=flag\nconst FLAG: bool = true;\n";
        let (migrated, count) = migrate_text(source, &config).unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            migrated,
            "// @anchor api.flag watch=line @strand api role=flag\nconst FLAG: bool = true;\n"
        );
    }
}
