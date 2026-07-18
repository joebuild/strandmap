use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{
    config::AnnotationConfig,
    model::{
        Anchor, Attributes, Diagnostic, IndexedMember, IndexedRelation, Location, Member,
        Provenance, Relation, Severity, Strand, WatchMode,
    },
    source_span,
};

#[derive(Debug, Default)]
pub struct AnnotationScan {
    pub anchors: Vec<(Anchor, Provenance)>,
    pub strands: Vec<(Strand, Provenance)>,
    pub memberships: Vec<IndexedMember>,
    pub relations: Vec<IndexedRelation>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
struct ActiveAnchor {
    id: String,
    line: usize,
}

type ParsedMembership = (Option<Strand>, (String, Member), Option<Anchor>);

#[derive(Debug, Clone, Copy)]
enum MarkerKind {
    Anchor,
    Strand,
    Relation,
}

pub fn scan_source(path: &str, text: &str, config: &AnnotationConfig) -> AnnotationScan {
    let mut result = AnnotationScan::default();
    if !config.enabled {
        return result;
    }
    let span_resolver = source_span::Resolver::new(path, text);
    let markers = markers(config);
    let mut active: Option<ActiveAnchor> = None;
    for (zero_line, line) in text.lines().enumerate() {
        let line_number = zero_line + 1;
        if active
            .as_ref()
            .is_some_and(|anchor| line_number.saturating_sub(anchor.line) > config.anchor_block_gap)
        {
            active = None;
        }
        let occurrences = marker_occurrences(line, &markers);
        for (index, (start, marker, kind)) in occurrences.iter().enumerate() {
            let value_start = start + marker.len();
            let value_end = occurrences.get(index + 1).map_or(line.len(), |item| item.0);
            let body = annotation_body(&line[value_start..value_end]);
            match kind {
                MarkerKind::Anchor => match parse_anchor(path, line_number, body) {
                    Ok(mut anchor) => {
                        if has_authored_source_location(body) {
                            result.diagnostics.push(Diagnostic {
                                severity: Severity::Warning,
                                code: "static-source-location".into(),
                                message: format!(
                                    "anchor {:?} authors source coordinates that can drift",
                                    anchor.id
                                ),
                                path: Some(path.into()),
                                line: u32::try_from(line_number).ok(),
                                hint: Some("run `strandmap migrate dynamic-locations`".into()),
                            });
                        }
                        if anchor.location.as_ref().and_then(|location| location.watch)
                            == Some(WatchMode::Node)
                        {
                            match span_resolver
                                .as_ref()
                                .map_err(Clone::clone)
                                .and_then(|resolver| resolver.resolve(line_number))
                            {
                                Ok(span) => {
                                    if let Some(location) = &mut anchor.location {
                                        location.line_start = Some(span.start_line);
                                        location.line_end = Some(span.end_line);
                                    }
                                }
                                Err(message) => result.diagnostics.push(annotation_error(
                                    path,
                                    line_number,
                                    "annotation-node",
                                    format!("anchor {:?}: {message}", anchor.id),
                                )),
                            }
                        }
                        active = Some(ActiveAnchor {
                            id: anchor.id.clone(),
                            line: line_number,
                        });
                        result
                            .anchors
                            .push((anchor, provenance(path, line_number, "annotation")));
                    }
                    Err(message) => result.diagnostics.push(annotation_error(
                        path,
                        line_number,
                        "annotation-anchor",
                        message,
                    )),
                },
                MarkerKind::Strand => {
                    match parse_membership(path, line_number, body, active.as_ref(), config) {
                        Ok((strand, member, implicit_anchor)) => {
                            if let Some(anchor) = implicit_anchor {
                                active = Some(ActiveAnchor {
                                    id: anchor.id.clone(),
                                    line: line_number,
                                });
                                result
                                    .anchors
                                    .push((anchor, provenance(path, line_number, "annotation")));
                            }
                            if let Some(strand_value) = strand {
                                result.strands.push((
                                    strand_value,
                                    provenance(path, line_number, "annotation"),
                                ));
                            }
                            result.memberships.push(IndexedMember {
                                strand: Some(member.0),
                                member: member.1,
                                provenance: provenance(path, line_number, "annotation"),
                            });
                        }
                        Err(message) => result.diagnostics.push(annotation_error(
                            path,
                            line_number,
                            "annotation-strand",
                            message,
                        )),
                    }
                }
                MarkerKind::Relation => match parse_relation(body, active.as_ref()) {
                    Ok((strand, relation)) => result.relations.push(IndexedRelation {
                        strand,
                        relation,
                        provenance: provenance(path, line_number, "annotation"),
                    }),
                    Err(message) => result.diagnostics.push(annotation_error(
                        path,
                        line_number,
                        "annotation-relation",
                        message,
                    )),
                },
            }
        }
    }
    result
}

fn annotation_body(value: &str) -> &str {
    let bytes = value.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
            index += 1;
            continue;
        }
        let remainder = &value[index..];
        if ["*/", "-->", "*)", "-/", "-}", "#>"]
            .iter()
            .any(|delimiter| remainder.starts_with(delimiter))
        {
            return value[..index].trim();
        }
        index += 1;
    }
    value.trim()
}

fn has_authored_source_location(body: &str) -> bool {
    tokens(body).is_ok_and(|(_, values)| {
        ["line", "lines", "line_start", "line_end"]
            .iter()
            .any(|key| values.contains_key(*key))
            || values
                .get("watch")
                .and_then(Value::as_str)
                .is_some_and(|watch| watch == "range")
    })
}

fn markers(config: &AnnotationConfig) -> Vec<(String, MarkerKind)> {
    let mut markers = Vec::new();
    markers.extend(
        config
            .anchor_markers
            .iter()
            .cloned()
            .map(|marker| (marker, MarkerKind::Anchor)),
    );
    markers.extend(
        config
            .strand_markers
            .iter()
            .cloned()
            .map(|marker| (marker, MarkerKind::Strand)),
    );
    markers.extend(
        config
            .relation_markers
            .iter()
            .cloned()
            .map(|marker| (marker, MarkerKind::Relation)),
    );
    markers.sort_by_key(|marker| std::cmp::Reverse(marker.0.len()));
    markers
}

fn marker_occurrences<'a>(
    line: &'a str,
    markers: &'a [(String, MarkerKind)],
) -> Vec<(usize, &'a str, MarkerKind)> {
    let mut found = Vec::new();
    for (marker, kind) in markers {
        let mut offset = 0;
        while let Some(relative) = line[offset..].find(marker) {
            let start = offset + relative;
            let after = start + marker.len();
            let boundary_before = start == 0
                || !line[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_');
            let boundary_after = after == line.len()
                || !line[after..]
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_');
            if boundary_before && boundary_after {
                found.push((start, marker.as_str(), *kind));
            }
            offset = after;
        }
    }
    found.sort_by_key(|item| item.0);
    let mut filtered = Vec::new();
    for item in found {
        if filtered
            .last()
            .is_none_or(|previous: &(usize, &str, MarkerKind)| {
                item.0 >= previous.0 + previous.1.len()
            })
        {
            filtered.push(item);
        }
    }
    filtered
}

fn parse_anchor(path: &str, line: usize, body: &str) -> Result<Anchor, String> {
    let (positionals, mut values) = tokens(body)?;
    reject_removed_fields(&values, &["description"])?;
    let id = positionals
        .first()
        .cloned()
        .or_else(|| take_string(&mut values, "id"))
        .ok_or_else(|| "anchor annotation requires an id".to_string())?;
    let mut location = Location {
        path: normalize_location_path(
            &take_string(&mut values, "path").unwrap_or_else(|| path.into()),
        )?,
        line_start: take_u32(&mut values, "line_start")?,
        line_end: take_u32(&mut values, "line_end")?,
        symbol: take_string(&mut values, "symbol"),
        language: take_string(&mut values, "language"),
        fingerprint: take_string(&mut values, "fingerprint"),
        watch: take_string(&mut values, "watch")
            .map(|value| parse_watch(&value))
            .transpose()?,
    };
    if let Some(lines) = take_string(&mut values, "lines") {
        let (start, end) = parse_range(&lines)?;
        location.line_start = Some(start);
        location.line_end = Some(end);
        location.watch.get_or_insert(WatchMode::Range);
    }
    let explicit_line = take_u32(&mut values, "line")?;
    if let Some(line_value) = explicit_line {
        location.line_start = Some(line_value);
        location.line_end.get_or_insert(line_value);
        location.watch.get_or_insert(WatchMode::Line);
    }
    if location.line_start.is_none() {
        let line = u32::try_from(line).map_err(|_| "line number exceeds u32".to_string())?;
        location.line_start = Some(line);
        location.line_end = Some(line);
    }
    location
        .watch
        .get_or_insert(if source_span::supports_dynamic(path) {
            WatchMode::Node
        } else {
            WatchMode::File
        });
    Ok(Anchor {
        schema: 1,
        id,
        target: take_string(&mut values, "target"),
        kind: take_string(&mut values, "kind"),
        location: Some(location),
        tags: take_set(&mut values, "tags"),
        attributes: values,
    })
}

fn parse_membership(
    path: &str,
    line: usize,
    body: &str,
    active: Option<&ActiveAnchor>,
    config: &AnnotationConfig,
) -> Result<ParsedMembership, String> {
    let (positionals, mut values) = tokens(body)?;
    reject_removed_fields(&values, &["rationale", "reason"])?;
    let strand_id = positionals
        .first()
        .cloned()
        .or_else(|| take_string(&mut values, "id"))
        .ok_or_else(|| "strand annotation requires a strand id".to_string())?;
    let explicit_anchor = take_string(&mut values, "anchor");
    let mut implicit = None;
    let anchor_id =
        if let Some(id) = explicit_anchor.or_else(|| active.map(|value| value.id.clone())) {
            id
        } else if config.implicit_anchors {
            let digest = blake3::hash(format!("{path}\0{body}").as_bytes()).to_hex();
            let id = format!("source:{path}:{}", &digest[..12]);
            let line = u32::try_from(line).map_err(|_| "line number exceeds u32".to_string())?;
            implicit = Some(Anchor {
                schema: 1,
                id: id.clone(),
                target: None,
                kind: take_string(&mut values, "kind"),
                location: Some(Location {
                    path: path.into(),
                    line_start: Some(line),
                    line_end: Some(line),
                    symbol: take_string(&mut values, "symbol"),
                    language: take_string(&mut values, "language"),
                    fingerprint: Some(digest.to_string()),
                    watch: Some(WatchMode::File),
                }),
                tags: BTreeSet::new(),
                attributes: Attributes::new(),
            });
            id
        } else {
            return Err(
                "strand annotation has no anchor; provide anchor=<id> or enable implicit_anchors"
                    .into(),
            );
        };
    let intent = take_string(&mut values, "intent");
    let strand = intent.map(|intent| Strand {
        schema: 1,
        id: strand_id.clone(),
        title: take_string(&mut values, "title"),
        intent,
        scope: take_string(&mut values, "scope"),
        tags: take_set(&mut values, "tags"),
        members: Vec::new(),
        relations: Vec::new(),
        on_change: None,
        attributes: Attributes::new(),
    });
    let required = take_bool(&mut values, "required")?.unwrap_or(true);
    Ok((
        strand,
        (
            strand_id,
            Member {
                anchor: anchor_id,
                role: take_string(&mut values, "role"),
                required,
                attributes: values,
            },
        ),
        implicit,
    ))
}

fn parse_relation(
    body: &str,
    active: Option<&ActiveAnchor>,
) -> Result<(Option<String>, Relation), String> {
    let (positionals, mut values) = tokens(body)?;
    reject_removed_fields(&values, &["rationale", "reason"])?;
    let kind = positionals
        .first()
        .cloned()
        .or_else(|| take_string(&mut values, "kind"))
        .or_else(|| take_string(&mut values, "type"))
        .ok_or_else(|| "relation annotation requires a relationship type".to_string())?;
    let from = take_string(&mut values, "from")
        .or_else(|| active.map(|anchor| anchor.id.clone()))
        .ok_or_else(|| {
            "relation annotation requires from=<anchor> or an active @anchor".to_string()
        })?;
    let to = take_string(&mut values, "to")
        .ok_or_else(|| "relation annotation requires to=<anchor>".to_string())?;
    let strand = take_string(&mut values, "strand");
    Ok((
        strand,
        Relation {
            from,
            to,
            kind,
            bidirectional: take_bool(&mut values, "bidirectional")?.unwrap_or(false),
            attributes: values,
        },
    ))
}

fn tokens(body: &str) -> Result<(Vec<String>, Attributes), String> {
    let tokens = shlex::split(body).ok_or_else(|| "unclosed quote in annotation".to_string())?;
    let mut positionals = Vec::new();
    let mut values = BTreeMap::new();
    for token in tokens {
        if let Some((key, value)) = token.split_once('=') {
            if key.is_empty() {
                return Err("annotation attribute name cannot be empty".into());
            }
            values.insert(key.to_string(), scalar(value));
        } else {
            positionals.push(token);
        }
    }
    Ok((positionals, values))
}

fn scalar(value: &str) -> Value {
    if value.eq_ignore_ascii_case("true") {
        Value::Bool(true)
    } else if value.eq_ignore_ascii_case("false") {
        Value::Bool(false)
    } else if value.eq_ignore_ascii_case("null") {
        Value::Null
    } else if let Ok(number) = value.parse::<i64>() {
        Value::Number(number.into())
    } else if let Ok(number) = value.parse::<f64>() {
        serde_json::Number::from_f64(number)
            .map_or_else(|| Value::String(value.into()), Value::Number)
    } else {
        Value::String(value.into())
    }
}

fn reject_removed_fields(values: &Attributes, names: &[&str]) -> Result<(), String> {
    if let Some(name) = names.iter().find(|name| values.contains_key(**name)) {
        return Err(format!("{name} metadata is not supported"));
    }
    Ok(())
}

fn take_string(values: &mut Attributes, name: &str) -> Option<String> {
    values.remove(name).map(|value| match value {
        Value::String(value) => value,
        other => other.to_string(),
    })
}

fn take_bool(values: &mut Attributes, name: &str) -> Result<Option<bool>, String> {
    let Some(value) = values.remove(name) else {
        return Ok(None);
    };
    match value {
        Value::Bool(value) => Ok(Some(value)),
        Value::String(value) if value.eq_ignore_ascii_case("true") => Ok(Some(true)),
        Value::String(value) if value.eq_ignore_ascii_case("false") => Ok(Some(false)),
        _ => Err(format!("{name} must be true or false")),
    }
}

fn take_u32(values: &mut Attributes, name: &str) -> Result<Option<u32>, String> {
    let Some(value) = values.remove(name) else {
        return Ok(None);
    };
    match value {
        Value::Number(number) => number
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .map(Some)
            .ok_or_else(|| format!("{name} must be a positive 32-bit integer")),
        Value::String(value) => value
            .parse::<u32>()
            .map(Some)
            .map_err(|_| format!("{name} must be a positive 32-bit integer")),
        _ => Err(format!("{name} must be a positive 32-bit integer")),
    }
}

fn take_set(values: &mut Attributes, name: &str) -> BTreeSet<String> {
    take_string(values, name)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_watch(value: &str) -> Result<WatchMode, String> {
    match value {
        "file" => Ok(WatchMode::File),
        "line" => Ok(WatchMode::Line),
        "range" => Ok(WatchMode::Range),
        "node" => Ok(WatchMode::Node),
        _ => Err(format!(
            "unknown watch mode {value:?}; use file, line, range, or node"
        )),
    }
}

fn parse_range(value: &str) -> Result<(u32, u32), String> {
    let normalized = value.trim_start_matches('L');
    let (start, end) = normalized
        .split_once('-')
        .map_or((normalized, normalized), |parts| parts);
    let start = start
        .parse::<u32>()
        .map_err(|_| format!("invalid line range {value:?}"))?;
    let end = end
        .parse::<u32>()
        .map_err(|_| format!("invalid line range {value:?}"))?;
    if start == 0 || end < start {
        return Err(format!("invalid line range {value:?}"));
    }
    Ok((start, end))
}

fn normalize_location_path(value: &str) -> Result<String, String> {
    let mut normalized = value.replace('\\', "/");
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.split('/').any(|part| part == "..")
    {
        return Err(
            "location paths must be repository-relative and may not escape the repository".into(),
        );
    }
    Ok(normalized)
}

fn provenance(path: &str, line: usize, source: &str) -> Provenance {
    Provenance {
        source: source.into(),
        path: Some(path.into()),
        line: u32::try_from(line).ok(),
    }
}

fn annotation_error(path: &str, line: usize, code: &str, message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: code.into(),
        message,
        path: Some(path.into()),
        line: u32::try_from(line).ok(),
        hint: Some("Run `strandmap check` after correcting the annotation".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_annotation_blocks_and_inline_relations() {
        let source = r#"
// @anchor auth.issue target=rust://auth::issue watch=range lines=2-8
// @strand token-format role=producer intent="Issuer and verifier agree"
fn issue() {}
// @anchor auth.verify @strand token-format role=consumer @relation mirrors to=auth.issue bidirectional=true
fn verify() {}
"#;
        let result = scan_source("src/auth.rs", source, &AnnotationConfig::default());
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].code, "static-source-location");
        assert_eq!(result.anchors.len(), 2);
        assert_eq!(result.memberships.len(), 2);
        assert_eq!(result.relations.len(), 1);
        assert_eq!(result.memberships[0].member.anchor, "auth.issue");
        assert_eq!(result.relations[0].relation.from, "auth.verify");
    }

    #[test]
    fn stops_inline_annotations_at_block_comment_boundaries() {
        let source = r#"
const mapped = values.map((value /* @anchor codec.map.value watch=line @strand data-contract role=input */) => value);
const checked = values.every((value /* @anchor codec.check.value watch=line @strand data-contract role=input */) => value > 0);
"#;
        let result = scan_source("src/index.js", source, &AnnotationConfig::default());
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.anchors.len(), 2);
        assert_eq!(result.memberships.len(), 2);
        assert_eq!(result.anchors[0].0.id, "codec.map.value");
        assert_eq!(result.memberships[1].member.anchor, "codec.check.value");
    }

    #[test]
    fn stops_inline_annotations_at_lean_block_comment_boundaries() {
        let source = r#"
def amountInTier (amount : Nat) /- @anchor arithmetic.amount watch=line @strand fee-model role=amount -/ (tier : FeeTier) := True
"#;
        let result = scan_source("formal/Spec.lean", source, &AnnotationConfig::default());
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.anchors.len(), 1);
        assert_eq!(result.memberships.len(), 1);
        assert_eq!(result.anchors[0].0.id, "arithmetic.amount");
        assert_eq!(result.memberships[0].member.role.as_deref(), Some("amount"));
    }

    #[test]
    fn preserves_comment_delimiters_inside_quoted_values() {
        let source = r#"/* @anchor docs.example target="docs://example/*/section" watch=line */"#;
        let result = scan_source("src/index.js", source, &AnnotationConfig::default());
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            result.anchors[0].0.target.as_deref(),
            Some("docs://example/*/section")
        );
    }

    #[test]
    fn creates_deterministic_implicit_anchor() {
        let source = "// @strand data-contract role=consumer";
        let first = scan_source("src/main.rs", source, &AnnotationConfig::default());
        let second = scan_source("src/main.rs", source, &AnnotationConfig::default());
        assert_eq!(
            first.memberships[0].member.anchor,
            second.memberships[0].member.anchor
        );
        assert!(
            first.memberships[0]
                .member
                .anchor
                .starts_with("source:src/main.rs:")
        );
    }

    #[test]
    fn rejects_removed_prose_fields() {
        let source = r#"
// @anchor auth.issue description="prose"
// @strand token-format rationale="prose"
// @relation mirrors to=auth.issue reason="prose"
"#;
        let result = scan_source("src/auth.rs", source, &AnnotationConfig::default());
        assert_eq!(result.diagnostics.len(), 3);
        assert!(
            result
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.message.contains("metadata is not supported"))
        );
    }
}
