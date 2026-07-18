use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::Arc,
};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::{
    model::{Index, Location, WatchMode},
    repo::Repository,
    source_span,
};

const CHUNK_LINES: usize = 56;
const CHUNK_OVERLAP: usize = 8;
const BLOOM_WORDS: usize = 32;

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub score: u32,
    pub matched: Vec<String>,
    pub anchors: Vec<String>,
}

impl SearchHit {
    #[must_use]
    pub fn reason(&self) -> String {
        format!(
            "match {}#L{}-L{}",
            self.path, self.line_start, self.line_end
        )
    }
}

#[derive(Debug)]
struct Query {
    terms: Vec<String>,
    excluded: BTreeSet<String>,
    phrases: Vec<Vec<String>>,
}

#[derive(Debug)]
struct Document {
    path: String,
    start: u32,
    end: u32,
    text: String,
    source: Arc<str>,
    tokens: Vec<String>,
    anchored: bool,
}

#[derive(Debug)]
struct RankedDocument {
    index: usize,
    score: f64,
    matched: BTreeSet<String>,
}

/// Locate relevant source units with BM25, then attach the graph anchors that
/// cover those units. Strand metadata deliberately does not participate in
/// relevance ranking; it is consulted only after source locations are found.
pub fn search(
    repo: &Repository,
    index: &Index,
    query: &str,
    limit: usize,
    paths: &[String],
    include_rust_tests: bool,
) -> Result<Vec<SearchHit>> {
    if limit == 0 {
        bail!("--search-limit must be greater than zero");
    }
    let query = parse_query(query)?;
    let mut documents = build_documents(repo, index, paths, &query.terms, include_rust_tests);
    if documents.is_empty() {
        return Ok(Vec::new());
    }
    let document_count = documents.len() as f64;
    let average_length = documents
        .iter()
        .map(|document| document.tokens.len())
        .sum::<usize>() as f64
        / document_count;
    let document_frequency = document_frequency(&documents, &query.terms);
    let idf: BTreeMap<_, _> = query
        .terms
        .iter()
        .map(|term| {
            let frequency = *document_frequency.get(term).unwrap_or(&0) as f64;
            let value = (1.0 + (document_count - frequency + 0.5) / (frequency + 0.5)).ln();
            (term.clone(), value)
        })
        .collect();
    let mut ranked = Vec::new();
    for (index, document) in documents.iter().enumerate() {
        if contains_excluded(document, &query.excluded) {
            continue;
        }
        let (score, matched) = bm25(document, &query, &idf, average_length);
        if score > 0.0 {
            ranked.push(RankedDocument {
                index,
                score,
                matched,
            });
        }
    }
    ranked.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| {
                documents[left.index]
                    .tokens
                    .len()
                    .cmp(&documents[right.index].tokens.len())
            })
            .then_with(|| documents[left.index].path.cmp(&documents[right.index].path))
            .then_with(|| {
                documents[left.index]
                    .start
                    .cmp(&documents[right.index].start)
            })
    });

    let mut hits: Vec<SearchHit> = Vec::new();
    for ranked in ranked {
        let document = &documents[ranked.index];
        let best_line = best_matching_line(document, &query, &idf);
        let span = if document.anchored {
            source_span::Span {
                start_line: document.start,
                end_line: document.end,
            }
        } else if let Some(span) = indexed_span_for_line(index, &document.path, best_line) {
            span
        } else {
            source_span::enclosing(
                &document.path,
                &document.source,
                usize::try_from(best_line).unwrap_or(usize::MAX),
            )
        };
        if hits.iter().any(|hit| {
            hit.path == document.path
                && ranges_substantially_overlap(
                    hit.line_start,
                    hit.line_end,
                    span.start_line,
                    span.end_line,
                )
        }) {
            continue;
        }
        let anchors = anchors_for_span(index, &document.path, span.start_line, span.end_line);
        hits.push(SearchHit {
            path: document.path.clone(),
            line_start: span.start_line,
            line_end: span.end_line,
            score: scaled_score(ranked.score),
            matched: ranked.matched.into_iter().collect(),
            anchors,
        });
        if hits.len() == limit {
            break;
        }
    }
    documents.clear();
    Ok(hits)
}

fn build_documents(
    repo: &Repository,
    index: &Index,
    paths: &[String],
    query_terms: &[String],
    include_rust_tests: bool,
) -> Vec<Document> {
    let mut by_path: BTreeMap<&str, Vec<(&str, &Location)>> = BTreeMap::new();
    for (id, anchor) in &index.anchors {
        if let Some(location) = anchor.value.location.as_ref() {
            by_path
                .entry(&location.path)
                .or_default()
                .push((id, location));
        }
    }
    let mut documents = Vec::new();
    for (path, record) in index.files.iter().filter(|(path, record)| {
        path_allowed(path, paths) && bloom_might_match(&record.search_bloom, query_terms)
    }) {
        let Ok(bytes) = fs::read(repo.root.join(path)) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let source: Arc<str> = Arc::from(text.as_str());
        let lines: Vec<_> = text.lines().collect();
        if lines.is_empty() {
            continue;
        }
        let test_ranges = if include_rust_tests {
            Vec::new()
        } else {
            record
                .rust_test_ranges
                .iter()
                .map(|range| source_span::Span {
                    start_line: range.start,
                    end_line: range.end,
                })
                .collect()
        };
        let mut seen = BTreeSet::new();
        if let Some(anchors) = by_path.get(path.as_str()) {
            for (_, location) in anchors {
                if location.watch == Some(WatchMode::File) {
                    continue;
                }
                let Some(start) = location.line_start else {
                    continue;
                };
                let end = location.line_end.unwrap_or(start);
                if start == 0 || end < start || start as usize > lines.len() {
                    continue;
                }
                let end = end.min(u32::try_from(lines.len()).unwrap_or(u32::MAX));
                if test_ranges
                    .iter()
                    .any(|range| range.start_line <= end && start <= range.end_line)
                {
                    continue;
                }
                if seen.insert((start, end)) {
                    push_document(
                        &mut documents,
                        path,
                        &lines,
                        &source,
                        &test_ranges,
                        source_span::Span {
                            start_line: start,
                            end_line: end,
                        },
                        true,
                    );
                }
            }
        }
        let step = CHUNK_LINES - CHUNK_OVERLAP;
        let mut start = 1usize;
        loop {
            let end = (start + CHUNK_LINES - 1).min(lines.len());
            let start_u32 = u32::try_from(start).unwrap_or(u32::MAX);
            let end_u32 = u32::try_from(end).unwrap_or(u32::MAX);
            if seen.insert((start_u32, end_u32)) {
                push_document(
                    &mut documents,
                    path,
                    &lines,
                    &source,
                    &test_ranges,
                    source_span::Span {
                        start_line: start_u32,
                        end_line: end_u32,
                    },
                    false,
                );
            }
            if end == lines.len() {
                break;
            }
            start += step;
        }
    }
    documents
}

fn push_document(
    documents: &mut Vec<Document>,
    path: &str,
    lines: &[&str],
    source: &Arc<str>,
    test_ranges: &[source_span::Span],
    span: source_span::Span,
    anchored: bool,
) {
    let start = span.start_line;
    let end = span.end_line;
    let start_index = usize::try_from(start.saturating_sub(1)).unwrap_or(usize::MAX);
    let end_index = usize::try_from(end).unwrap_or(usize::MAX).min(lines.len());
    if start_index >= end_index {
        return;
    }
    let text = lines[start_index..end_index]
        .iter()
        .enumerate()
        .map(|(offset, line)| {
            let line_number = start.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
            if test_ranges
                .iter()
                .any(|range| range.start_line <= line_number && line_number <= range.end_line)
            {
                ""
            } else {
                strip_annotations(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let tokens = normalize_tokens(&text);
    if !tokens.is_empty() {
        documents.push(Document {
            path: path.to_string(),
            start,
            end,
            text,
            source: Arc::clone(source),
            tokens,
            anchored,
        });
    }
}

fn strip_annotations(line: &str) -> &str {
    ["@strandmap", "@anchor", "@strand"]
        .into_iter()
        .filter_map(|marker| line.find(marker))
        .min()
        .map_or(line, |index| &line[..index])
}

pub(crate) fn source_bloom(text: &str) -> Vec<u64> {
    let mut bloom = vec![0u64; BLOOM_WORDS];
    for line in text.lines() {
        for token in normalize_tokens(strip_annotations(line)) {
            bloom_insert(&mut bloom, &token);
        }
    }
    bloom
}

fn bloom_might_match(bloom: &[u64], terms: &[String]) -> bool {
    bloom.len() != BLOOM_WORDS || terms.iter().any(|term| bloom_contains(bloom, term))
}

fn bloom_insert(bloom: &mut [u64], token: &str) {
    for bit in bloom_bits(token) {
        bloom[bit / 64] |= 1u64 << (bit % 64);
    }
}

fn bloom_contains(bloom: &[u64], token: &str) -> bool {
    bloom_bits(token)
        .into_iter()
        .all(|bit| bloom[bit / 64] & (1u64 << (bit % 64)) != 0)
}

fn bloom_bits(token: &str) -> [usize; 2] {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in token.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mixed = hash.rotate_left(29).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let bits = BLOOM_WORDS * 64;
    [(hash as usize) % bits, (mixed as usize) % bits]
}

fn document_frequency(documents: &[Document], terms: &[String]) -> BTreeMap<String, usize> {
    let wanted: BTreeSet<_> = terms.iter().collect();
    let mut frequencies = BTreeMap::new();
    for document in documents {
        let present: BTreeSet<_> = document
            .tokens
            .iter()
            .filter(|token| wanted.contains(token))
            .collect();
        for term in present {
            *frequencies.entry(term.clone()).or_default() += 1;
        }
    }
    frequencies
}

fn bm25(
    document: &Document,
    query: &Query,
    idf: &BTreeMap<String, f64>,
    average_length: f64,
) -> (f64, BTreeSet<String>) {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let mut frequencies = BTreeMap::new();
    for token in &document.tokens {
        *frequencies.entry(token.as_str()).or_insert(0usize) += 1;
    }
    let mut score = 0.0;
    let mut matched = BTreeSet::new();
    for term in &query.terms {
        let frequency = *frequencies.get(term.as_str()).unwrap_or(&0) as f64;
        if frequency == 0.0 {
            continue;
        }
        let length_ratio = document.tokens.len() as f64 / average_length.max(1.0);
        let denominator = frequency + K1 * (1.0 - B + B * length_ratio);
        score +=
            idf.get(term).copied().unwrap_or_default() * (frequency * (K1 + 1.0) / denominator);
        matched.insert(term.clone());
    }
    let normalized = document.tokens.join(" ");
    for phrase in &query.phrases {
        let phrase = phrase.join(" ");
        if normalized.contains(&phrase) {
            let phrase_idf = phrase
                .split_whitespace()
                .filter_map(|term| idf.get(term))
                .sum::<f64>();
            score += phrase_idf * 0.75;
        }
    }
    let coverage = matched.len() as f64 / query.terms.len().max(1) as f64;
    (score * (0.35 + 0.65 * coverage), matched)
}

fn best_matching_line(document: &Document, query: &Query, idf: &BTreeMap<String, f64>) -> u32 {
    document
        .text
        .lines()
        .enumerate()
        .map(|(offset, line)| {
            let tokens: BTreeSet<_> = normalize_tokens(line).into_iter().collect();
            let score = query
                .terms
                .iter()
                .filter(|term| tokens.contains(*term))
                .map(|term| idf.get(term).copied().unwrap_or_default())
                .sum::<f64>();
            (score, offset)
        })
        .max_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| right.1.cmp(&left.1))
        })
        .map_or(document.start, |(_, offset)| {
            document
                .start
                .saturating_add(u32::try_from(offset).unwrap_or(u32::MAX))
        })
}

fn anchors_for_span(index: &Index, path: &str, start: u32, end: u32) -> Vec<String> {
    let mut bounded = Vec::new();
    let mut file = Vec::new();
    for (id, anchor) in &index.anchors {
        let Some(location) = anchor.value.location.as_ref() else {
            continue;
        };
        if location.path != path {
            continue;
        }
        if location.watch == Some(WatchMode::File) {
            file.push(id.clone());
            continue;
        }
        let Some(anchor_start) = location.line_start else {
            continue;
        };
        let anchor_end = location.line_end.unwrap_or(anchor_start);
        if anchor_start <= end && start <= anchor_end {
            bounded.push(id.clone());
        }
    }
    if bounded.is_empty() { file } else { bounded }
}

fn indexed_span_for_line(index: &Index, path: &str, line: u32) -> Option<source_span::Span> {
    index
        .anchors
        .values()
        .filter_map(|anchor| {
            let location = anchor.value.location.as_ref()?;
            if location.path != path || location.watch == Some(WatchMode::File) {
                return None;
            }
            let start = location.line_start?;
            let end = location.line_end.unwrap_or(start);
            (start <= line && line <= end).then_some(source_span::Span {
                start_line: start,
                end_line: end,
            })
        })
        .min_by_key(|span| span.end_line.saturating_sub(span.start_line))
}

fn contains_excluded(document: &Document, excluded: &BTreeSet<String>) -> bool {
    document.tokens.iter().any(|token| excluded.contains(token))
}

fn ranges_substantially_overlap(
    left_start: u32,
    left_end: u32,
    right_start: u32,
    right_end: u32,
) -> bool {
    let overlap_start = left_start.max(right_start);
    let overlap_end = left_end.min(right_end);
    if overlap_start > overlap_end {
        return false;
    }
    let overlap = overlap_end - overlap_start + 1;
    let shortest = (left_end - left_start + 1).min(right_end - right_start + 1);
    overlap.saturating_mul(2) >= shortest
}

fn scaled_score(score: f64) -> u32 {
    let score = (score * 1_000.0).round();
    if score.is_finite() && score > 0.0 {
        score.min(f64::from(u32::MAX)) as u32
    } else {
        0
    }
}

fn path_allowed(path: &str, paths: &[String]) -> bool {
    paths.is_empty()
        || paths.iter().any(|selected| {
            path == selected
                || path
                    .strip_prefix(selected)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
}

fn parse_query(query: &str) -> Result<Query> {
    let words = query_words(query)?;
    if words.is_empty() {
        bail!("search query cannot be empty");
    }
    let mut terms = Vec::new();
    let mut excluded = BTreeSet::new();
    let mut phrases = Vec::new();
    for (mut word, quoted) in words {
        let is_excluded = word.starts_with('-');
        if is_excluded {
            word.remove(0);
        }
        let tokens = normalize_tokens(&word);
        if tokens.is_empty() {
            bail!("search term {word:?} has no searchable characters");
        }
        if is_excluded {
            excluded.extend(tokens);
        } else {
            if quoted && tokens.len() > 1 {
                phrases.push(tokens.clone());
            }
            terms.extend(tokens);
        }
    }
    if terms.len() > 1 {
        terms.retain(|term| !is_task_scaffolding(term));
    }
    terms.sort();
    terms.dedup();
    if terms.is_empty() {
        bail!("search query must contain at least one meaningful term");
    }
    Ok(Query {
        terms,
        excluded,
        phrases,
    })
}

fn is_task_scaffolding(value: &str) -> bool {
    matches!(
        value,
        "a" | "add"
            | "an"
            | "and"
            | "build"
            | "change"
            | "create"
            | "feature"
            | "fix"
            | "for"
            | "implement"
            | "in"
            | "make"
            | "new"
            | "of"
            | "on"
            | "or"
            | "please"
            | "support"
            | "the"
            | "to"
            | "update"
            | "with"
    )
}

fn query_words(query: &str) -> Result<Vec<(String, bool)>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut quoted = false;
    let mut escaped = false;
    for character in query.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            quoted = true;
        } else if character.is_whitespace() {
            if !current.is_empty() {
                words.push((std::mem::take(&mut current), quoted));
                quoted = false;
            }
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    if quote.is_some() {
        bail!("unterminated quote in search query");
    }
    if !current.is_empty() {
        words.push((current, quoted));
    }
    Ok(words)
}

fn normalize_tokens(value: &str) -> Vec<String> {
    let mut expanded = String::with_capacity(value.len() + 8);
    let mut previous_lower_or_digit = false;
    for character in value.chars() {
        if character.is_uppercase() && previous_lower_or_digit {
            expanded.push(' ');
        }
        if character.is_alphanumeric() {
            for lowercase in character.to_lowercase() {
                expanded.push(lowercase);
            }
        } else {
            expanded.push(' ');
        }
        previous_lower_or_digit = character.is_lowercase() || character.is_ascii_digit();
    }
    expanded.split_whitespace().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parser_keeps_phrases_and_exclusions() {
        let query = parse_query("customer \"profile avatar\" -legacy").unwrap();
        assert_eq!(query.terms, ["avatar", "customer", "profile"]);
        assert_eq!(query.phrases, [vec!["profile", "avatar"]]);
        assert!(query.excluded.contains("legacy"));
    }

    #[test]
    fn identifiers_are_split_without_losing_acronyms() {
        assert_eq!(
            normalize_tokens("CustomerProfile_avatar::v2"),
            ["customer", "profile", "avatar", "v2"]
        );
    }

    #[test]
    fn natural_task_scaffolding_does_not_crowd_feature_terms() {
        let query = parse_query("please add customer profile avatar").unwrap();
        assert_eq!(query.terms, ["avatar", "customer", "profile"]);
        assert_eq!(parse_query("add").unwrap().terms, ["add"]);
    }

    #[test]
    fn overlap_rejects_nested_chunks_but_keeps_separate_functions() {
        assert!(ranges_substantially_overlap(10, 20, 12, 16));
        assert!(!ranges_substantially_overlap(10, 20, 21, 30));
    }

    #[test]
    fn bloom_filters_absent_terms_without_indexing_annotations() {
        let bloom =
            source_bloom("// @anchor metadata.internal-only\npub fn render_profile_avatar() {}\n");
        assert!(bloom_might_match(&bloom, &["avatar".to_string()]));
        assert!(!bloom_might_match(&bloom, &["internal".to_string()]));
    }
}
