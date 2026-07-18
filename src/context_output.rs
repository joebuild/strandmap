use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::{
    cli::ContextSource,
    model::{AffectedAnchor, ContextPacket, Location, Severity, WatchMode},
    repo::Repository,
    search::SearchHit,
    source_span,
};

#[derive(Debug, Clone, Copy)]
pub struct RenderOptions<'a> {
    pub source: ContextSource,
    pub context_lines: u32,
    pub merge_gap: u32,
    pub token_budget: Option<usize>,
    pub focused_anchors: &'a BTreeMap<String, usize>,
    pub source_matches: &'a [SearchHit],
    pub include_rust_tests: bool,
    pub source_includes: &'a [String],
    pub source_excludes: &'a [String],
    pub source_extensions: &'a [String],
    pub excluded_source_extensions: &'a [String],
    pub source_languages: &'a [String],
    pub excluded_source_languages: &'a [String],
}

struct SourceFilters {
    includes: GlobSet,
    excludes: GlobSet,
    has_includes: bool,
    has_excludes: bool,
    extensions: BTreeSet<String>,
    excluded_extensions: BTreeSet<String>,
    languages: BTreeSet<String>,
    excluded_languages: BTreeSet<String>,
}

#[derive(Default)]
struct FilteredSource {
    ranges: BTreeSet<(String, String, u32, u32)>,
}

#[derive(Default)]
struct OmittedRustTests {
    ranges: BTreeSet<(String, u32, u32)>,
}

#[derive(Debug, Clone)]
struct Request {
    path: String,
    start: u32,
    end: u32,
    priority: u8,
    order: usize,
    language: Option<String>,
    anchor_ids: BTreeSet<String>,
    references: BTreeSet<String>,
    matched: bool,
}

#[derive(Debug)]
struct Excerpt {
    path: String,
    start: u32,
    end: u32,
    priority: u8,
    order: usize,
    language: Option<String>,
    anchor_ids: BTreeSet<String>,
    references: BTreeSet<String>,
    source: String,
    matched: bool,
}

pub fn render(
    repo: &Repository,
    packet: &ContextPacket,
    options: RenderOptions<'_>,
) -> Result<String> {
    let filters = SourceFilters::new(&options)?;
    let (requests, filtered_source) = collect_requests(packet, &options, &filters);
    let (mut excerpts, mut omitted, omitted_rust_tests) = load_excerpts(
        repo,
        requests,
        options.merge_gap,
        options.include_rust_tests,
    )?;
    excerpts.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then(left.order.cmp(&right.order))
            .then(left.path.cmp(&right.path))
            .then(left.start.cmp(&right.start))
    });
    let mut remaining = options.token_budget;
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    let mut blocked_priority = None;
    let mut tier_priority = None;
    let mut tier_included = false;
    let mut tier_excluded = false;
    for excerpt in excerpts {
        if tier_priority != Some(excerpt.priority) {
            if tier_included && tier_excluded {
                blocked_priority = tier_priority;
            }
            tier_priority = Some(excerpt.priority);
            tier_included = false;
            tier_excluded = false;
        }
        if blocked_priority.is_some() {
            excluded.push(excerpt);
            continue;
        }
        let estimated = estimate_tokens(excerpt.source.len() + 64);
        if remaining.is_some_and(|budget| estimated > budget) {
            tier_excluded = true;
            excluded.push(excerpt);
            continue;
        }
        if let Some(budget) = &mut remaining {
            *budget = budget.saturating_sub(estimated);
        }
        tier_included = true;
        included.push(excerpt);
    }
    let mut included = merge_included_excerpts(included);
    let mut budget_omitted_count = 0usize;
    let mut budget_omitted_tokens = 0usize;
    let mut budget_omitted_paths: BTreeMap<String, usize> = BTreeMap::new();
    for excerpt in excluded {
        if let Some(covering) = included.iter_mut().find(|included| {
            included.path == excerpt.path
                && included.start <= excerpt.start
                && included.end >= excerpt.end
        }) {
            covering.anchor_ids.extend(excerpt.anchor_ids);
            covering.references.extend(excerpt.references);
            covering.matched |= excerpt.matched;
            continue;
        }
        budget_omitted_count += 1;
        budget_omitted_tokens += estimate_tokens(excerpt.source.len() + 64);
        *budget_omitted_paths.entry(excerpt.path).or_default() += 1;
    }
    included.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then(left.order.cmp(&right.order))
            .then(left.path.cmp(&right.path))
            .then(left.start.cmp(&right.start))
    });
    const MAX_DETAILED_ANCHORS: usize = 4;

    let mut output = String::new();
    writeln!(output, "# Strandmap context")?;
    writeln!(output, "Selection: {}", packet.changes.description)?;
    if !packet.changes.files.is_empty() {
        writeln!(output, "Changed files: {}", packet.changes.files.len())?;
        for file in &packet.changes.files {
            let ranges = if file.whole_file || file.ranges.is_empty() {
                "whole-file".to_string()
            } else {
                file.ranges
                    .iter()
                    .map(|range| format!("{}-{}", range.start, range.end))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            writeln!(output, "- {} ({:?}; {ranges})", file.path, file.status)?;
        }
    }

    writeln!(output, "\n## Strands ({})", packet.strands.len())?;
    if packet.strands.is_empty() {
        writeln!(output, "No strands selected.")?;
    }
    for strand in &packet.strands {
        let marker = if strand.direct { "direct" } else { "connected" };
        writeln!(output, "\n### {} [{marker}]", strand.id)?;
        writeln!(output, "Intent: {}", strand.intent)?;
        let mut hidden = Vec::new();
        let mut shown = 0usize;
        for anchor in &strand.anchors {
            let focused = options.focused_anchors.contains_key(&anchor.id);
            if (anchor.direct || focused) && shown < MAX_DETAILED_ANCHORS {
                write_anchor_line(&mut output, anchor)?;
                shown += 1;
            } else {
                hidden.push(anchor);
            }
        }
        if !hidden.is_empty() {
            write_hidden_anchor_summary(&mut output, &hidden, shown > 0)?;
        }
    }
    if !packet.related_anchors.is_empty() {
        writeln!(
            output,
            "\n## Related anchors ({})",
            packet.related_anchors.len()
        )?;
        let mut hidden = Vec::new();
        let mut shown = 0usize;
        for anchor in &packet.related_anchors {
            let focused = options.focused_anchors.contains_key(&anchor.id);
            if (anchor.direct || focused) && shown < MAX_DETAILED_ANCHORS {
                write_anchor_line(&mut output, anchor)?;
                shown += 1;
            } else {
                hidden.push(anchor);
            }
        }
        if !hidden.is_empty() {
            write_hidden_anchor_summary(&mut output, &hidden, shown > 0)?;
        }
    }
    if !packet.unmatched_files.is_empty() {
        writeln!(output, "\n## Unmatched changed files")?;
        for path in &packet.unmatched_files {
            writeln!(output, "- {path}")?;
        }
    }

    if !matches!(options.source, ContextSource::None) {
        writeln!(output, "\n## Source excerpts ({})", included.len())?;
        if !options.include_rust_tests {
            writeln!(
                output,
                "Rust test sections are omitted by default; use `--include-tests` to include them."
            )?;
        }
        for (index, excerpt) in included.iter().enumerate() {
            let marker = match excerpt.priority {
                0 if excerpt.matched => "match",
                0 => "focused",
                1 => "direct",
                2 => "selected-strand",
                _ => "candidate",
            };
            writeln!(
                output,
                "\n### E{} {}#L{}-L{} [{marker}]",
                index + 1,
                excerpt.path,
                excerpt.start,
                excerpt.end
            )?;
            let fence = code_fence(&excerpt.source);
            writeln!(
                output,
                "{fence}{}",
                excerpt.language.as_deref().unwrap_or_default()
            )?;
            writeln!(output, "{}", excerpt.source)?;
            writeln!(output, "{fence}")?;
        }
    }
    if !filtered_source.ranges.is_empty()
        || !omitted_rust_tests.ranges.is_empty()
        || budget_omitted_count > 0
        || !omitted.is_empty()
    {
        omitted.sort();
        omitted.dedup();
        writeln!(output, "\n## Omitted source excerpts")?;
        if !filtered_source.ranges.is_empty() {
            write_filtered_source_summary(&mut output, &filtered_source)?;
        }
        if !omitted_rust_tests.ranges.is_empty() {
            let files: BTreeSet<_> = omitted_rust_tests
                .ranges
                .iter()
                .map(|(path, _, _)| path)
                .collect();
            writeln!(
                output,
                "- Omitted {} Rust test sections across {} files.",
                omitted_rust_tests.ranges.len(),
                files.len()
            )?;
        }
        if budget_omitted_count > 0 {
            writeln!(
                output,
                "- Context budget omitted {budget_omitted_count} complete source excerpts (~{budget_omitted_tokens} tokens) across {} files.",
                budget_omitted_paths.len()
            )?;
            const MAX_PATH_SUMMARIES: usize = 8;
            for (path, count) in budget_omitted_paths.iter().take(MAX_PATH_SUMMARIES) {
                writeln!(output, "  - {path}: {count}")?;
            }
            if budget_omitted_paths.len() > MAX_PATH_SUMMARIES {
                writeln!(
                    output,
                    "  - … {} additional files",
                    budget_omitted_paths.len() - MAX_PATH_SUMMARIES
                )?;
            }
        }
        for item in omitted {
            writeln!(output, "- {item}")?;
        }
    }

    let diagnostics: Vec<_> = packet
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity != Severity::Info)
        .collect();
    if !diagnostics.is_empty() {
        writeln!(output, "\n## Diagnostics")?;
        const MAX_BOUNDED_DIAGNOSTICS: usize = 20;
        let shown = diagnostics.len().min(MAX_BOUNDED_DIAGNOSTICS);
        for diagnostic in diagnostics.iter().take(shown) {
            let path = diagnostic.path.as_deref().unwrap_or("-");
            let line = diagnostic
                .line
                .map_or_else(String::new, |line| format!(":{line}"));
            writeln!(
                output,
                "- {:?} [{}] {path}{line}: {}",
                diagnostic.severity, diagnostic.code, diagnostic.message
            )?;
        }
        if shown < diagnostics.len() {
            writeln!(
                output,
                "- … {} additional diagnostics omitted by context budget",
                diagnostics.len() - shown
            )?;
        }
    }
    Ok(output)
}

impl SourceFilters {
    fn new(options: &RenderOptions<'_>) -> Result<Self> {
        Ok(Self {
            includes: compile_globs(options.source_includes, "--source-include")?,
            excludes: compile_globs(options.source_excludes, "--source-exclude")?,
            has_includes: !options.source_includes.is_empty(),
            has_excludes: !options.source_excludes.is_empty(),
            extensions: normalize_extensions(options.source_extensions)?,
            excluded_extensions: normalize_extensions(options.excluded_source_extensions)?,
            languages: normalize_languages(options.source_languages)?,
            excluded_languages: normalize_languages(options.excluded_source_languages)?,
        })
    }

    fn allows(&self, path: &str, language: Option<&str>) -> bool {
        if (self.has_includes && !self.includes.is_match(path))
            || (self.has_excludes && self.excludes.is_match(path))
        {
            return false;
        }
        let extension = extension_for_path(path);
        if (!self.extensions.is_empty()
            && extension
                .as_ref()
                .is_none_or(|extension| !self.extensions.contains(extension)))
            || extension
                .as_ref()
                .is_some_and(|extension| self.excluded_extensions.contains(extension))
        {
            return false;
        }
        let language = language.map(normalize_language);
        if (!self.languages.is_empty()
            && language
                .as_ref()
                .is_none_or(|language| !self.languages.contains(language)))
            || language
                .as_ref()
                .is_some_and(|language| self.excluded_languages.contains(language))
        {
            return false;
        }
        true
    }
}

fn compile_globs(patterns: &[String], option: &str) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        if pattern.trim().is_empty() {
            bail!("{option} cannot be empty");
        }
        builder
            .add(Glob::new(pattern).with_context(|| format!("invalid {option} glob {pattern:?}"))?);
    }
    builder
        .build()
        .with_context(|| format!("failed to compile {option} globs"))
}

fn normalize_extensions(values: &[String]) -> Result<BTreeSet<String>> {
    values
        .iter()
        .map(|value| {
            let value = value.trim().trim_start_matches('.').to_ascii_lowercase();
            if value.is_empty() || value.contains(['/', '\\']) {
                bail!("invalid source extension {value:?}");
            }
            Ok(value)
        })
        .collect()
}

fn normalize_languages(values: &[String]) -> Result<BTreeSet<String>> {
    values
        .iter()
        .map(|value| {
            let value = normalize_language(value);
            if value.is_empty() {
                bail!("source language cannot be empty");
            }
            Ok(value)
        })
        .collect()
}

fn normalize_language(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "rs" => "rust".to_string(),
        "js" | "mjs" | "cjs" => "javascript".to_string(),
        "ts" | "mts" | "cts" => "typescript".to_string(),
        "py" => "python".to_string(),
        "sh" | "zsh" => "bash".to_string(),
        "md" => "markdown".to_string(),
        "yml" => "yaml".to_string(),
        language => language.to_string(),
    }
}

fn extension_for_path(path: &str) -> Option<String> {
    let path = Path::new(path);
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .or_else(|| {
            let name = path.file_name()?.to_str()?;
            name.strip_prefix('.')
                .filter(|name| !name.is_empty() && !name.contains('.'))
                .map(str::to_ascii_lowercase)
        })
}

fn write_filtered_source_summary(output: &mut String, filtered: &FilteredSource) -> Result<()> {
    let mut paths: BTreeMap<&str, usize> = BTreeMap::new();
    for (path, _, _, _) in &filtered.ranges {
        *paths.entry(path).or_default() += 1;
    }
    writeln!(
        output,
        "- Source filters excluded {} anchored ranges across {} files.",
        filtered.ranges.len(),
        paths.len()
    )?;
    const MAX_PATH_SUMMARIES: usize = 8;
    for (path, count) in paths.iter().take(MAX_PATH_SUMMARIES) {
        writeln!(output, "  - {path}: {count}")?;
    }
    if paths.len() > MAX_PATH_SUMMARIES {
        writeln!(
            output,
            "  - … {} additional files",
            paths.len() - MAX_PATH_SUMMARIES
        )?;
    }
    Ok(())
}

fn write_hidden_anchor_summary(
    output: &mut String,
    anchors: &[&AffectedAnchor],
    has_details: bool,
) -> Result<()> {
    let files: BTreeSet<_> = anchors
        .iter()
        .filter_map(|anchor| {
            anchor
                .anchor
                .as_ref()
                .and_then(|anchor| anchor.location.as_ref())
                .map(|location| location.path.as_str())
        })
        .collect();
    if files.is_empty() {
        writeln!(
            output,
            "- … {} {}anchors summarized",
            anchors.len(),
            if has_details { "additional " } else { "" }
        )?;
    } else {
        writeln!(
            output,
            "- … {} {}anchors across {} files summarized",
            anchors.len(),
            if has_details { "additional " } else { "" },
            files.len()
        )?;
    }
    Ok(())
}

fn write_anchor_line(output: &mut String, anchor: &AffectedAnchor) -> Result<()> {
    let direct = if anchor.direct { "direct" } else { "candidate" };
    let role = anchor.role.as_deref().unwrap_or("member");
    let location = anchor
        .anchor
        .as_ref()
        .and_then(|anchor| anchor.location.as_ref())
        .map_or_else(|| "unlocated".to_string(), display_location);
    writeln!(output, "- [{direct}] {role}: {} — {location}", anchor.id)?;
    Ok(())
}

fn display_location(location: &Location) -> String {
    match (location.line_start, location.line_end) {
        (Some(start), Some(end)) if start != end => format!("{}#L{start}-L{end}", location.path),
        (Some(line), _) => format!("{}#L{line}", location.path),
        _ => location.path.clone(),
    }
}

fn collect_requests(
    packet: &ContextPacket,
    options: &RenderOptions<'_>,
    filters: &SourceFilters,
) -> (Vec<Request>, FilteredSource) {
    if matches!(options.source, ContextSource::None) {
        return (Vec::new(), FilteredSource::default());
    }
    let mut requests = Vec::new();
    if matches!(options.source, ContextSource::Direct | ContextSource::All) {
        for (order, hit) in options.source_matches.iter().enumerate() {
            requests.push(Request {
                path: hit.path.clone(),
                start: hit.line_start.saturating_sub(options.context_lines).max(1),
                end: hit.line_end.saturating_add(options.context_lines),
                priority: 0,
                order,
                language: language_for_path(&hit.path).map(str::to_string),
                anchor_ids: hit.anchors.iter().cloned().collect(),
                references: BTreeSet::from(["search".to_string()]),
                matched: true,
            });
        }
    }
    for strand in &packet.strands {
        for anchor in &strand.anchors {
            let focus_order = options.focused_anchors.get(&anchor.id).copied();
            let priority = if focus_order.is_some() {
                0
            } else if anchor.direct {
                1
            } else if strand.direct {
                2
            } else {
                3
            };
            push_request(
                &mut requests,
                anchor,
                priority,
                focus_order.unwrap_or(usize::MAX),
                anchor.direct || strand.direct,
                format!(
                    "{}/{}:{}",
                    strand.id,
                    anchor.role.as_deref().unwrap_or("member"),
                    anchor.id
                ),
                options,
            );
        }
    }
    for anchor in &packet.related_anchors {
        let focus_order = options.focused_anchors.get(&anchor.id).copied();
        push_request(
            &mut requests,
            anchor,
            if focus_order.is_some() {
                0
            } else if anchor.direct {
                1
            } else {
                3
            },
            focus_order.unwrap_or(usize::MAX),
            anchor.direct,
            format!("related:{}", anchor.id),
            options,
        );
    }
    suppress_redundant_whole_files(&mut requests);
    let mut filtered = FilteredSource::default();
    requests.retain(|request| {
        if filters.allows(&request.path, request.language.as_deref()) {
            true
        } else {
            for anchor in &request.anchor_ids {
                filtered.ranges.insert((
                    request.path.clone(),
                    anchor.clone(),
                    request.start,
                    request.end,
                ));
            }
            false
        }
    });
    (requests, filtered)
}

fn push_request(
    requests: &mut Vec<Request>,
    anchor: &AffectedAnchor,
    priority: u8,
    order: usize,
    direct_source: bool,
    reference: String,
    options: &RenderOptions<'_>,
) {
    if (matches!(options.source, ContextSource::Focused) && priority != 0)
        || (matches!(options.source, ContextSource::Direct) && !direct_source)
    {
        return;
    }
    let Some(location) = anchor
        .anchor
        .as_ref()
        .and_then(|anchor| anchor.location.as_ref())
    else {
        return;
    };
    let (start, end) = if location.watch == Some(WatchMode::File) {
        (1, u32::MAX)
    } else {
        (
            location
                .line_start
                .unwrap_or(1)
                .saturating_sub(options.context_lines)
                .max(1),
            location
                .line_end
                .or(location.line_start)
                .unwrap_or(u32::MAX)
                .saturating_add(options.context_lines),
        )
    };
    requests.push(Request {
        path: location.path.clone(),
        start,
        end,
        priority,
        order,
        language: location
            .language
            .clone()
            .or_else(|| language_for_path(&location.path).map(str::to_string)),
        anchor_ids: BTreeSet::from([anchor.id.clone()]),
        references: BTreeSet::from([reference]),
        matched: false,
    });
}

fn suppress_redundant_whole_files(requests: &mut Vec<Request>) {
    let bounded_paths: BTreeSet<_> = requests
        .iter()
        .filter(|request| request.end != u32::MAX)
        .map(|request| request.path.clone())
        .collect();
    requests.retain(|request| request.end != u32::MAX || !bounded_paths.contains(&request.path));
}

fn load_excerpts(
    repo: &Repository,
    requests: Vec<Request>,
    merge_gap: u32,
    include_rust_tests: bool,
) -> Result<(Vec<Excerpt>, Vec<String>, OmittedRustTests)> {
    let mut by_path: BTreeMap<String, Vec<Request>> = BTreeMap::new();
    for request in requests {
        by_path
            .entry(request.path.clone())
            .or_default()
            .push(request);
    }
    let mut excerpts = Vec::new();
    let mut omitted = Vec::new();
    let mut omitted_rust_tests = OmittedRustTests::default();
    for (path, mut requests) in by_path {
        requests
            .sort_by_key(|request| (request.priority, request.order, request.start, request.end));
        let mut merged: Vec<Request> = Vec::new();
        for request in requests {
            if let Some(previous) = merged.last_mut() {
                if request.priority == previous.priority
                    && request.start <= previous.end.saturating_add(merge_gap).saturating_add(1)
                {
                    previous.end = previous.end.max(request.end);
                    previous.priority = previous.priority.min(request.priority);
                    previous.order = previous.order.min(request.order);
                    previous.anchor_ids.extend(request.anchor_ids);
                    previous.references.extend(request.references);
                    previous.matched |= request.matched;
                    if previous.language.is_none() {
                        previous.language = request.language;
                    }
                    continue;
                }
            }
            merged.push(request);
        }
        let absolute = repo.root.join(&path);
        let bytes = match fs::read(&absolute) {
            Ok(bytes) => bytes,
            Err(error) => {
                omitted.push(format!("{path} (read failed: {error})"));
                continue;
            }
        };
        if bytes.contains(&0) {
            omitted.push(format!("{path} (binary file)"));
            continue;
        }
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                omitted.push(format!("{path} (not UTF-8)"));
                continue;
            }
        };
        let lines: Vec<_> = text.lines().collect();
        let available = u32::try_from(lines.len()).unwrap_or(u32::MAX);
        let test_ranges = if include_rust_tests {
            Vec::new()
        } else {
            source_span::rust_test_ranges(&path, &text)
        };
        let requests = merged
            .into_iter()
            .flat_map(|request| {
                subtract_rust_tests(
                    request,
                    &test_ranges,
                    available,
                    &path,
                    &mut omitted_rust_tests,
                )
            })
            .collect::<Vec<_>>();
        for request in requests {
            if request.start > available || lines.is_empty() {
                omitted.push(format!(
                    "{path}#L{}-L{} (range is outside file)",
                    request.start, request.end
                ));
                continue;
            }
            let end = request.end.min(available);
            let start_index = usize::try_from(request.start.saturating_sub(1))
                .context("source line does not fit usize")?;
            let end_index = usize::try_from(end).context("source line does not fit usize")?;
            let source = lines[start_index..end_index].join("\n");
            if source.trim().is_empty() {
                continue;
            }
            excerpts.push(Excerpt {
                path: path.clone(),
                start: request.start,
                end,
                priority: request.priority,
                order: request.order,
                language: request.language,
                anchor_ids: request.anchor_ids,
                references: request.references,
                source,
                matched: request.matched,
            });
        }
    }
    Ok((excerpts, omitted, omitted_rust_tests))
}

fn subtract_rust_tests(
    request: Request,
    test_ranges: &[source_span::Span],
    available: u32,
    path: &str,
    omitted: &mut OmittedRustTests,
) -> Vec<Request> {
    if available == 0 || request.start > available {
        return vec![request];
    }
    let end = request.end.min(available);
    let mut cursor = request.start;
    let mut output = Vec::new();
    for range in test_ranges
        .iter()
        .filter(|range| range.start_line <= end && request.start <= range.end_line)
    {
        omitted
            .ranges
            .insert((path.to_string(), range.start_line, range.end_line));
        if cursor < range.start_line {
            let mut production = request.clone();
            production.start = cursor;
            production.end = range.start_line - 1;
            output.push(production);
        }
        cursor = cursor.max(range.end_line.saturating_add(1));
        if cursor > end {
            break;
        }
    }
    if cursor <= end {
        let mut production = request;
        production.start = cursor;
        production.end = end;
        output.push(production);
    }
    output
}

fn merge_included_excerpts(mut excerpts: Vec<Excerpt>) -> Vec<Excerpt> {
    excerpts.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.start.cmp(&right.start))
            .then(left.order.cmp(&right.order))
            .then(left.end.cmp(&right.end))
    });
    let mut merged: Vec<Excerpt> = Vec::new();
    for excerpt in excerpts {
        if let Some(previous) = merged.last_mut() {
            if excerpt.path == previous.path && excerpt.start <= previous.end.saturating_add(1) {
                if excerpt.end > previous.end {
                    let overlapping_lines =
                        previous.end.saturating_add(1).saturating_sub(excerpt.start) as usize;
                    let tail = excerpt
                        .source
                        .lines()
                        .skip(overlapping_lines)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !tail.is_empty() {
                        if !previous.source.is_empty() {
                            previous.source.push('\n');
                        }
                        previous.source.push_str(&tail);
                    }
                    previous.end = excerpt.end;
                }
                previous.priority = previous.priority.min(excerpt.priority);
                previous.order = previous.order.min(excerpt.order);
                previous.anchor_ids.extend(excerpt.anchor_ids);
                previous.references.extend(excerpt.references);
                previous.matched |= excerpt.matched;
                if previous.language.is_none() {
                    previous.language = excerpt.language;
                }
                continue;
            }
        }
        merged.push(excerpt);
    }
    merged
}

fn estimate_tokens(characters: usize) -> usize {
    characters.div_ceil(4)
}

fn code_fence(source: &str) -> String {
    let longest = source
        .lines()
        .flat_map(|line| line.split(|character| character != '`'))
        .map(str::len)
        .max()
        .unwrap_or_default();
    "`".repeat(longest.saturating_add(1).max(3))
}

fn language_for_path(path: &str) -> Option<&'static str> {
    match Path::new(path).extension()?.to_str()? {
        "rs" => Some("rust"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("jsx"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "py" => Some("python"),
        "sh" | "bash" | "zsh" => Some("bash"),
        "md" => Some("markdown"),
        "yaml" | "yml" => Some("yaml"),
        "json" => Some("json"),
        "toml" => Some("toml"),
        "lean" => Some("lean"),
        "tla" => Some("tla"),
        "sol" => Some("solidity"),
        _ => None,
    }
}
