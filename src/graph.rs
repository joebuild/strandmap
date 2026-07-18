use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use crate::{
    annotations,
    config::Config,
    metadata,
    model::{
        AffectedAnchor, AffectedStrand, ChangeSet, ContextPacket, Index, IndexedRelation, Location,
        Member, Relation, Strand, WatchMode,
    },
    search::SearchHit,
};

#[derive(Debug, Clone, Default)]
pub struct AffectedOptions {
    pub depth: Option<usize>,
    pub relations: BTreeSet<String>,
    pub tags: BTreeSet<String>,
    pub include_optional: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub anchors: Vec<QueryAnchor>,
    pub strands: Vec<QueryStrand>,
    pub relations: Vec<IndexedRelation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryAnchor {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryStrand {
    pub id: String,
    pub intent: String,
}

#[derive(Debug, Clone)]
struct AnchorReason {
    reason: String,
    direct: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ContextSeeds {
    pub description: String,
    pub anchors: BTreeMap<String, String>,
    pub strands: BTreeMap<String, String>,
    pub search_anchors: BTreeSet<String>,
    pub source_matches: Vec<SearchHit>,
}

pub fn affected(
    index: &Index,
    changes: ChangeSet,
    config: &Config,
    options: &AffectedOptions,
) -> ContextPacket {
    let mut reasons: BTreeMap<String, AnchorReason> = BTreeMap::new();
    let mut matched_files = BTreeSet::new();
    let mut direct_strand_seeds = BTreeSet::new();
    let mut removed_strands: BTreeMap<String, Strand> = BTreeMap::new();
    let mut removed_members: Vec<(String, Member)> = Vec::new();
    let mut removed_anchors = BTreeMap::new();
    let mut removed_relations: Vec<Relation> = Vec::new();

    for (strand_id, strand) in &index.strands {
        if let Some(changed) = changed_metadata_for(&changes, &strand.provenance) {
            direct_strand_seeds.insert(strand_id.clone());
            matched_files.insert(changed.path.clone());
        }
    }
    for membership in &index.memberships {
        let Some(changed) = changed_metadata_for(&changes, &membership.provenance) else {
            continue;
        };
        if let Some(strand) = &membership.strand {
            direct_strand_seeds.insert(strand.clone());
        }
        matched_files.insert(changed.path.clone());
        reasons.insert(
            membership.member.anchor.clone(),
            AnchorReason {
                reason: format!(
                    "metadata changed in {}",
                    membership.provenance.path.as_deref().unwrap_or("sidecar")
                ),
                direct: true,
            },
        );
    }
    for relation in &index.relations {
        let Some(changed) = changed_metadata_for(&changes, &relation.provenance) else {
            continue;
        };
        if let Some(strand) = &relation.strand {
            direct_strand_seeds.insert(strand.clone());
        }
        matched_files.insert(changed.path.clone());
        for anchor in [&relation.relation.from, &relation.relation.to] {
            reasons.insert(
                anchor.clone(),
                AnchorReason {
                    reason: format!(
                        "relationship metadata changed in {}",
                        relation.provenance.path.as_deref().unwrap_or("sidecar")
                    ),
                    direct: true,
                },
            );
        }
    }
    for (anchor_id, indexed) in &index.anchors {
        if let Some(changed) = changed_metadata_for(&changes, &indexed.provenance) {
            let path = indexed.provenance.path.as_deref().unwrap_or("sidecar");
            matched_files.insert(changed.path.clone());
            reasons.insert(
                anchor_id.clone(),
                AnchorReason {
                    reason: format!("anchor metadata changed in {path}"),
                    direct: true,
                },
            );
        }
        let Some(location) = &indexed.value.location else {
            continue;
        };
        for changed in &changes.files {
            if location.path != changed.path
                && changed.old_path.as_deref() != Some(location.path.as_str())
            {
                continue;
            }
            if anchor_matches(location, changed) {
                matched_files.insert(changed.path.clone());
                reasons.insert(
                    anchor_id.clone(),
                    AnchorReason {
                        reason: format!("{} changed", changed.path),
                        direct: true,
                    },
                );
            }
        }
    }

    let mut removed_by_path: BTreeMap<&str, Vec<_>> = BTreeMap::new();
    for line in &changes.removed_lines {
        removed_by_path.entry(&line.path).or_default().push(line);
    }
    for (path, mut lines) in removed_by_path {
        lines.sort_by_key(|line| line.line);
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let deleted = changes.files.iter().any(|file| {
            file.status == crate::model::ChangeStatus::Deleted
                && (file.path == path || file.old_path.as_deref() == Some(path))
        });
        if deleted {
            if let Ok(parsed) = metadata::parse_metadata_text(std::path::Path::new(path), &text) {
                matched_files.insert(path.into());
                for strand in parsed.strands {
                    direct_strand_seeds.insert(strand.id.clone());
                    for member in &strand.members {
                        reasons.insert(
                            member.anchor.clone(),
                            AnchorReason {
                                reason: format!("strand metadata removed from {path}"),
                                direct: true,
                            },
                        );
                        removed_members.push((strand.id.clone(), member.clone()));
                    }
                    for relation in &strand.relations {
                        removed_relations.push(relation.clone());
                    }
                    removed_strands.insert(strand.id.clone(), strand);
                }
                for anchor in parsed.anchors {
                    reasons.insert(
                        anchor.id.clone(),
                        AnchorReason {
                            reason: format!("anchor metadata removed from {path}"),
                            direct: true,
                        },
                    );
                    removed_anchors.insert(anchor.id.clone(), anchor);
                }
                for relation in parsed.relations {
                    for anchor in [&relation.from, &relation.to] {
                        reasons.insert(
                            anchor.clone(),
                            AnchorReason {
                                reason: format!("relationship metadata removed from {path}"),
                                direct: true,
                            },
                        );
                    }
                    removed_relations.push(relation);
                }
                continue;
            }
        }
        let scan = annotations::scan_source(path, &text, &config.annotations);
        for (strand, _) in scan.strands {
            removed_strands.insert(strand.id.clone(), strand);
        }
        for (anchor, _) in scan.anchors {
            removed_anchors.insert(anchor.id.clone(), anchor);
        }
        for membership in scan.memberships {
            if let Some(strand) = membership.strand {
                direct_strand_seeds.insert(strand.clone());
                removed_strands
                    .entry(strand.clone())
                    .or_insert_with(|| Strand {
                        schema: 1,
                        id: strand.clone(),
                        title: None,
                        intent: String::new(),
                        scope: None,
                        tags: BTreeSet::new(),
                        members: Vec::new(),
                        relations: Vec::new(),
                        on_change: None,
                        attributes: BTreeMap::new(),
                    });
                removed_members.push((strand, membership.member.clone()));
                reasons
                    .entry(membership.member.anchor)
                    .or_insert(AnchorReason {
                        reason: format!("annotation removed from {path}"),
                        direct: true,
                    });
                matched_files.insert(path.into());
            }
        }
    }

    let direct_anchors: BTreeSet<String> = reasons.keys().cloned().collect();
    let mut active_strands = direct_strand_seeds;
    for membership in &index.memberships {
        if direct_anchors.contains(&membership.member.anchor) {
            if let Some(strand) = &membership.strand {
                active_strands.insert(strand.clone());
            }
        }
    }
    active_strands.retain(|id| {
        strand_selected(index, id, &options.tags)
            || removed_strands
                .get(id)
                .is_some_and(|strand| options.tags.iter().all(|tag| strand.tags.contains(tag)))
    });
    let direct_strands = active_strands.clone();

    let include_optional = options
        .include_optional
        .unwrap_or(config.traversal.include_optional_members);
    expand_strand_members(index, &active_strands, &mut reasons, include_optional);

    let depth = options.depth.unwrap_or_else(|| {
        direct_strands
            .iter()
            .filter_map(|id| index.strands.get(id))
            .filter_map(|strand| strand.value.on_change.as_ref()?.depth)
            .max()
            .unwrap_or(config.traversal.depth)
    });
    let relation_filter = if options.relations.is_empty() {
        &config.traversal.relation_kinds
    } else {
        &options.relations
    };
    let mut frontier: BTreeSet<String> = reasons.keys().cloned().collect();
    let mut visited = frontier.clone();
    for _ in 0..depth {
        let mut next = BTreeSet::new();
        for relation in &index.relations {
            if !affected_relation_allowed(
                index,
                relation,
                relation_filter,
                options.relations.is_empty(),
            ) {
                continue;
            }
            let from_seen = frontier.contains(&relation.relation.from);
            let to_seen = frontier.contains(&relation.relation.to);
            if from_seen && !visited.contains(&relation.relation.to) {
                next.insert((
                    relation.relation.to.clone(),
                    format!(
                        "{} relationship from {}",
                        relation.relation.kind, relation.relation.from
                    ),
                ));
            }
            if to_seen && !visited.contains(&relation.relation.from) {
                next.insert((
                    relation.relation.from.clone(),
                    format!(
                        "{} relationship to {}",
                        relation.relation.kind, relation.relation.to
                    ),
                ));
            }
        }
        for relation in &removed_relations {
            if !relation_filter.is_empty() && !relation_filter.contains(&relation.kind) {
                continue;
            }
            let from_seen = frontier.contains(&relation.from);
            let to_seen = frontier.contains(&relation.to);
            if from_seen && !visited.contains(&relation.to) {
                next.insert((
                    relation.to.clone(),
                    format!("{} relationship from {}", relation.kind, relation.from),
                ));
            }
            if to_seen && !visited.contains(&relation.from) {
                next.insert((
                    relation.from.clone(),
                    format!("{} relationship to {}", relation.kind, relation.to),
                ));
            }
        }
        if next.is_empty() {
            break;
        }
        frontier.clear();
        for (anchor, reason) in next {
            visited.insert(anchor.clone());
            frontier.insert(anchor.clone());
            reasons.entry(anchor).or_insert(AnchorReason {
                reason,
                direct: false,
            });
        }
        let newly_connected: BTreeSet<String> = index
            .memberships
            .iter()
            .filter(|membership| frontier.contains(&membership.member.anchor))
            .filter_map(|membership| membership.strand.clone())
            .filter(|id| strand_selected(index, id, &options.tags))
            .collect();
        active_strands.extend(newly_connected.iter().cloned());
        expand_strand_members(index, &newly_connected, &mut reasons, include_optional);
        for anchor in reasons.keys() {
            if visited.insert(anchor.clone()) {
                frontier.insert(anchor.clone());
            }
        }
    }

    let mut strands = Vec::new();
    for strand_id in &active_strands {
        let strand = index
            .strands
            .get(strand_id)
            .map(|strand| &strand.value)
            .or_else(|| removed_strands.get(strand_id));
        let Some(strand) = strand else { continue };
        let mut anchors = Vec::new();
        let current_members = index
            .memberships
            .iter()
            .filter(|membership| membership.strand.as_deref() == Some(strand_id))
            .map(|membership| &membership.member);
        let deleted_members = removed_members
            .iter()
            .filter(|(id, _)| id == strand_id)
            .map(|(_, member)| member);
        for member in current_members.chain(deleted_members) {
            if !member_selected(member, strand, include_optional) {
                continue;
            }
            let reason = reasons
                .get(&member.anchor)
                .cloned()
                .unwrap_or(AnchorReason {
                    reason: format!("member of strand {strand_id}"),
                    direct: false,
                });
            anchors.push(AffectedAnchor {
                id: member.anchor.clone(),
                reason: reason.reason,
                direct: reason.direct,
                role: member.role.clone(),
                anchor: index
                    .anchors
                    .get(&member.anchor)
                    .map(|anchor| anchor.value.clone())
                    .or_else(|| removed_anchors.get(&member.anchor).cloned()),
            });
        }
        anchors.sort_by(|left, right| left.id.cmp(&right.id).then(left.role.cmp(&right.role)));
        anchors.dedup_by(|left, right| left.id == right.id && left.role == right.role);
        strands.push(AffectedStrand {
            id: strand_id.clone(),
            intent: strand.intent.clone(),
            direct: direct_strands.contains(strand_id),
            anchors,
        });
    }
    let represented: BTreeSet<_> = strands
        .iter()
        .flat_map(|strand| strand.anchors.iter().map(|anchor| anchor.id.as_str()))
        .collect();
    let related_anchors = reasons
        .iter()
        .filter(|(id, _)| !represented.contains(id.as_str()))
        .map(|(id, reason)| AffectedAnchor {
            id: id.clone(),
            reason: reason.reason.clone(),
            direct: reason.direct,
            role: None,
            anchor: index
                .anchors
                .get(id)
                .map(|anchor| anchor.value.clone())
                .or_else(|| removed_anchors.get(id).cloned()),
        })
        .collect();
    let unmatched_files = changes
        .files
        .iter()
        .filter(|file| !matched_files.contains(&file.path))
        .map(|file| file.path.clone())
        .collect();
    ContextPacket {
        changes,
        strands,
        related_anchors,
        unmatched_files,
        diagnostics: index.diagnostics.clone(),
    }
}

pub fn context_from_seeds(
    index: &Index,
    seeds: &ContextSeeds,
    config: &Config,
    options: &AffectedOptions,
) -> ContextPacket {
    let mut reasons: BTreeMap<String, AnchorReason> = seeds
        .anchors
        .iter()
        .map(|(id, reason)| {
            (
                id.clone(),
                AnchorReason {
                    reason: reason.clone(),
                    direct: true,
                },
            )
        })
        .collect();
    let direct_anchors: BTreeSet<_> = seeds.anchors.keys().cloned().collect();
    let mut active_strands: BTreeSet<_> = seeds.strands.keys().cloned().collect();
    for membership in &index.memberships {
        if direct_anchors.contains(&membership.member.anchor) {
            if let Some(strand) = &membership.strand {
                active_strands.insert(strand.clone());
            }
        }
    }
    active_strands.retain(|id| strand_selected(index, id, &options.tags));
    let direct_strands: BTreeSet<_> = seeds.strands.keys().cloned().collect();
    let mut expanded_strands = direct_strands.clone();
    for membership in &index.memberships {
        if direct_anchors.contains(&membership.member.anchor)
            && !seeds.search_anchors.contains(&membership.member.anchor)
        {
            if let Some(strand) = &membership.strand {
                expanded_strands.insert(strand.clone());
            }
        }
    }
    let include_optional = options
        .include_optional
        .unwrap_or(config.traversal.include_optional_members);
    expand_strand_members(index, &expanded_strands, &mut reasons, include_optional);

    let depth = options.depth.unwrap_or(config.traversal.depth);
    let relation_filter = if options.relations.is_empty() {
        &config.traversal.relation_kinds
    } else {
        &options.relations
    };
    let mut frontier: BTreeSet<_> = reasons.keys().cloned().collect();
    let mut visited = frontier.clone();
    for _ in 0..depth {
        let mut next = BTreeSet::new();
        for relation in &index.relations {
            if !relation_filter.is_empty() && !relation_filter.contains(&relation.relation.kind) {
                continue;
            }
            let from_seen = frontier.contains(&relation.relation.from);
            let to_seen = frontier.contains(&relation.relation.to);
            if from_seen && !visited.contains(&relation.relation.to) {
                next.insert((
                    relation.relation.to.clone(),
                    format!(
                        "{} relationship from {}",
                        relation.relation.kind, relation.relation.from
                    ),
                ));
            }
            if to_seen && !visited.contains(&relation.relation.from) {
                next.insert((
                    relation.relation.from.clone(),
                    format!(
                        "{} relationship to {}",
                        relation.relation.kind, relation.relation.to
                    ),
                ));
            }
        }
        if next.is_empty() {
            break;
        }
        frontier.clear();
        for (anchor, reason) in next {
            visited.insert(anchor.clone());
            frontier.insert(anchor.clone());
            reasons.entry(anchor).or_insert(AnchorReason {
                reason,
                direct: false,
            });
        }
        let newly_connected: BTreeSet<_> = index
            .memberships
            .iter()
            .filter(|membership| frontier.contains(&membership.member.anchor))
            .filter_map(|membership| membership.strand.clone())
            .filter(|id| strand_selected(index, id, &options.tags))
            .collect();
        active_strands.extend(newly_connected.iter().cloned());
        if seeds.search_anchors.is_empty() {
            expand_strand_members(index, &newly_connected, &mut reasons, include_optional);
            expanded_strands.extend(newly_connected);
        }
        for anchor in reasons.keys() {
            if visited.insert(anchor.clone()) {
                frontier.insert(anchor.clone());
            }
        }
    }

    let mut strands = Vec::new();
    for strand_id in &active_strands {
        let Some(strand) = index.strands.get(strand_id).map(|strand| &strand.value) else {
            continue;
        };
        let mut anchors = Vec::new();
        for membership in index
            .memberships
            .iter()
            .filter(|membership| membership.strand.as_deref() == Some(strand_id))
        {
            if !member_selected(&membership.member, strand, include_optional) {
                continue;
            }
            if !expanded_strands.contains(strand_id)
                && !reasons.contains_key(&membership.member.anchor)
            {
                continue;
            }
            let reason = reasons
                .get(&membership.member.anchor)
                .cloned()
                .unwrap_or(AnchorReason {
                    reason: format!("member of strand {strand_id}"),
                    direct: false,
                });
            anchors.push(AffectedAnchor {
                id: membership.member.anchor.clone(),
                reason: reason.reason,
                direct: reason.direct,
                role: membership.member.role.clone(),
                anchor: index
                    .anchors
                    .get(&membership.member.anchor)
                    .map(|anchor| anchor.value.clone()),
            });
        }
        anchors.sort_by(|left, right| left.id.cmp(&right.id).then(left.role.cmp(&right.role)));
        anchors.dedup_by(|left, right| left.id == right.id && left.role == right.role);
        strands.push(AffectedStrand {
            id: strand_id.clone(),
            intent: strand.intent.clone(),
            direct: direct_strands.contains(strand_id),
            anchors,
        });
    }
    let represented: BTreeSet<_> = strands
        .iter()
        .flat_map(|strand| strand.anchors.iter().map(|anchor| anchor.id.as_str()))
        .collect();
    let related_anchors = reasons
        .iter()
        .filter(|(id, _)| !represented.contains(id.as_str()))
        .map(|(id, reason)| AffectedAnchor {
            id: id.clone(),
            reason: reason.reason.clone(),
            direct: reason.direct,
            role: None,
            anchor: index.anchors.get(id).map(|anchor| anchor.value.clone()),
        })
        .collect();
    let description = if seeds.description.is_empty() {
        "explicit graph context".to_string()
    } else {
        seeds.description.clone()
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(description.as_bytes());
    for (id, reason) in &seeds.anchors {
        hasher.update(id.as_bytes());
        hasher.update(reason.as_bytes());
    }
    for (id, reason) in &seeds.strands {
        hasher.update(id.as_bytes());
        hasher.update(reason.as_bytes());
    }
    ContextPacket {
        changes: ChangeSet {
            description,
            fingerprint: hasher.finalize().to_hex().to_string(),
            files: Vec::new(),
            removed_lines: Vec::new(),
        },
        strands,
        related_anchors,
        unmatched_files: Vec::new(),
        diagnostics: index.diagnostics.clone(),
    }
}

pub fn merge_context(mut primary: ContextPacket, additional: ContextPacket) -> ContextPacket {
    if !additional.changes.description.is_empty() {
        primary.changes.description = format!(
            "{}; {}",
            primary.changes.description, additional.changes.description
        );
        primary.changes.fingerprint = blake3::hash(
            format!(
                "{}:{}",
                primary.changes.fingerprint, additional.changes.fingerprint
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
    }
    for incoming in additional.strands {
        if let Some(existing) = primary
            .strands
            .iter_mut()
            .find(|strand| strand.id == incoming.id)
        {
            existing.direct |= incoming.direct;
            if existing.intent.is_empty() {
                existing.intent = incoming.intent;
            }
            for anchor in incoming.anchors {
                merge_anchor(&mut existing.anchors, anchor);
            }
            existing
                .anchors
                .sort_by(|left, right| left.id.cmp(&right.id).then(left.role.cmp(&right.role)));
        } else {
            primary.strands.push(incoming);
        }
    }
    primary
        .strands
        .sort_by(|left, right| left.id.cmp(&right.id));
    for anchor in additional.related_anchors {
        merge_anchor(&mut primary.related_anchors, anchor);
    }
    let represented: BTreeSet<_> = primary
        .strands
        .iter()
        .flat_map(|strand| strand.anchors.iter().map(|anchor| anchor.id.as_str()))
        .collect();
    primary
        .related_anchors
        .retain(|anchor| !represented.contains(anchor.id.as_str()));
    primary
        .related_anchors
        .sort_by(|left, right| left.id.cmp(&right.id).then(left.role.cmp(&right.role)));
    for diagnostic in additional.diagnostics {
        if !primary.diagnostics.contains(&diagnostic) {
            primary.diagnostics.push(diagnostic);
        }
    }
    primary
}

fn merge_anchor(anchors: &mut Vec<AffectedAnchor>, incoming: AffectedAnchor) {
    if let Some(existing) = anchors
        .iter_mut()
        .find(|anchor| anchor.id == incoming.id && anchor.role == incoming.role)
    {
        if incoming.direct && !existing.direct {
            existing.reason = incoming.reason;
        } else if incoming.reason != existing.reason && !existing.reason.contains(&incoming.reason)
        {
            existing.reason = format!("{}; {}", existing.reason, incoming.reason);
        }
        existing.direct |= incoming.direct;
        if existing.anchor.is_none() {
            existing.anchor = incoming.anchor;
        }
    } else {
        anchors.push(incoming);
    }
}

fn changed_for_path<'a>(
    changes: &'a ChangeSet,
    path: &str,
) -> Option<&'a crate::model::ChangedFile> {
    changes
        .files
        .iter()
        .find(|file| file.path == path || file.old_path.as_deref() == Some(path))
}

fn changed_metadata_for<'a>(
    changes: &'a ChangeSet,
    provenance: &crate::model::Provenance,
) -> Option<&'a crate::model::ChangedFile> {
    let changed = changed_for_path(changes, provenance.path.as_deref()?)?;
    match provenance.source.as_str() {
        "sidecar" => Some(changed),
        "annotation" => {
            let line = provenance.line?;
            (changed.whole_file
                || changed
                    .ranges
                    .iter()
                    .any(|range| range.overlaps(line, line)))
            .then_some(changed)
        }
        _ => None,
    }
}

fn affected_relation_allowed(
    index: &Index,
    relation: &IndexedRelation,
    global_filter: &BTreeSet<String>,
    respect_strand_policy: bool,
) -> bool {
    if !global_filter.is_empty() && !global_filter.contains(&relation.relation.kind) {
        return false;
    }
    if !respect_strand_policy {
        return true;
    }
    let Some(strand_id) = &relation.strand else {
        return true;
    };
    let Some(policy) = index
        .strands
        .get(strand_id)
        .and_then(|strand| strand.value.on_change.as_ref())
    else {
        return true;
    };
    policy.follow_relations.is_empty() || policy.follow_relations.contains(&relation.relation.kind)
}

fn anchor_matches(location: &crate::model::Location, changed: &crate::model::ChangedFile) -> bool {
    if changed.whole_file {
        return true;
    }
    match location.watch.unwrap_or(WatchMode::File) {
        WatchMode::File => true,
        WatchMode::Line | WatchMode::Range | WatchMode::Node => {
            let Some(start) = location.line_start else {
                return true;
            };
            let end = location.line_end.unwrap_or(start);
            changed
                .ranges
                .iter()
                .any(|range| range.overlaps(start, end))
        }
    }
}

fn strand_selected(index: &Index, id: &str, tags: &BTreeSet<String>) -> bool {
    tags.is_empty()
        || index
            .strands
            .get(id)
            .is_some_and(|strand| tags.iter().all(|tag| strand.value.tags.contains(tag)))
}

fn expand_strand_members(
    index: &Index,
    strands: &BTreeSet<String>,
    reasons: &mut BTreeMap<String, AnchorReason>,
    include_optional: bool,
) {
    for strand_id in strands {
        let Some(strand) = index.strands.get(strand_id) else {
            continue;
        };
        for membership in index
            .memberships
            .iter()
            .filter(|membership| membership.strand.as_deref() == Some(strand_id))
        {
            if member_selected(&membership.member, &strand.value, include_optional) {
                reasons
                    .entry(membership.member.anchor.clone())
                    .or_insert(AnchorReason {
                        reason: format!("member of strand {strand_id}"),
                        direct: false,
                    });
            }
        }
    }
}

fn member_selected(member: &Member, strand: &crate::model::Strand, include_optional: bool) -> bool {
    if !member.required && !include_optional {
        return false;
    }
    let Some(policy) = &strand.on_change else {
        return true;
    };
    if !policy.include_roles.is_empty()
        && member
            .role
            .as_ref()
            .is_none_or(|role| !policy.include_roles.contains(role))
    {
        return false;
    }
    if member
        .role
        .as_ref()
        .is_some_and(|role| policy.exclude_roles.contains(role))
    {
        return false;
    }
    true
}

pub fn query(
    index: &Index,
    anchor_seeds: &BTreeSet<String>,
    strand_seeds: &BTreeSet<String>,
    depth: usize,
    relation_filter: &BTreeSet<String>,
) -> QueryResult {
    let mut anchors = anchor_seeds.clone();
    let mut strands = strand_seeds.clone();
    for membership in &index.memberships {
        if membership
            .strand
            .as_ref()
            .is_some_and(|strand| strands.contains(strand))
        {
            anchors.insert(membership.member.anchor.clone());
        }
    }
    let mut frontier: VecDeque<(String, usize)> =
        anchors.iter().cloned().map(|anchor| (anchor, 0)).collect();
    let mut selected_relations = Vec::new();
    let mut relation_keys = BTreeSet::new();
    while let Some((anchor, level)) = frontier.pop_front() {
        if level >= depth {
            continue;
        }
        for relation in &index.relations {
            if !relation_filter.is_empty() && !relation_filter.contains(&relation.relation.kind) {
                continue;
            }
            let neighbor = if relation.relation.from == anchor {
                Some(&relation.relation.to)
            } else if relation.relation.to == anchor {
                Some(&relation.relation.from)
            } else {
                None
            };
            let Some(neighbor) = neighbor else {
                continue;
            };
            let key = (
                relation.relation.from.clone(),
                relation.relation.to.clone(),
                relation.relation.kind.clone(),
            );
            if relation_keys.insert(key) {
                selected_relations.push(relation.clone());
            }
            if anchors.insert(neighbor.clone()) {
                frontier.push_back((neighbor.clone(), level + 1));
            }
        }
    }
    for membership in &index.memberships {
        if anchors.contains(&membership.member.anchor) {
            if let Some(strand) = &membership.strand {
                strands.insert(strand.clone());
            }
        }
    }
    QueryResult {
        anchors: anchors
            .into_iter()
            .map(|id| QueryAnchor {
                kind: index
                    .anchors
                    .get(&id)
                    .and_then(|anchor| anchor.value.kind.clone()),
                location: index
                    .anchors
                    .get(&id)
                    .and_then(|anchor| anchor.value.location.clone()),
                id,
            })
            .collect(),
        strands: strands
            .into_iter()
            .map(|id| QueryStrand {
                intent: index
                    .strands
                    .get(&id)
                    .map_or_else(String::new, |strand| strand.value.intent.clone()),
                id,
            })
            .collect(),
        relations: selected_relations,
    }
}

pub fn to_dot(result: &QueryResult) -> String {
    let mut output = String::from("digraph strandmap {\n  rankdir=LR;\n");
    for anchor in &result.anchors {
        output.push_str(&format!("  \"{}\";\n", escape_dot(&anchor.id)));
    }
    for relation in &result.relations {
        let arrow = if relation.relation.bidirectional {
            "both"
        } else {
            "forward"
        };
        output.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\", dir={}];\n",
            escape_dot(&relation.relation.from),
            escape_dot(&relation.relation.to),
            escape_dot(&relation.relation.kind),
            arrow
        ));
    }
    output.push_str("}\n");
    output
}

pub fn to_mermaid(result: &QueryResult) -> String {
    let mut output = String::from("flowchart LR\n");
    let ids: BTreeMap<&str, String> = result
        .anchors
        .iter()
        .enumerate()
        .map(|(index, anchor)| (anchor.id.as_str(), format!("a{index}")))
        .collect();
    for anchor in &result.anchors {
        output.push_str(&format!(
            "  {}[\"{}\"]\n",
            ids[anchor.id.as_str()],
            escape_mermaid(&anchor.id)
        ));
    }
    for relation in &result.relations {
        let Some(from) = ids.get(relation.relation.from.as_str()) else {
            continue;
        };
        let Some(to) = ids.get(relation.relation.to.as_str()) else {
            continue;
        };
        let arrow = if relation.relation.bidirectional {
            "<-->"
        } else {
            "-->"
        };
        output.push_str(&format!(
            "  {from} {arrow}|{}| {to}\n",
            escape_mermaid(&relation.relation.kind)
        ));
    }
    output
}

fn escape_dot(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_mermaid(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChangeStatus, ChangedFile, LineRange, Location};

    #[test]
    fn range_matching_is_precise_but_file_mode_is_conservative() {
        let changed = ChangedFile {
            path: "src/lib.rs".into(),
            old_path: None,
            status: ChangeStatus::Modified,
            ranges: vec![LineRange { start: 20, end: 22 }],
            whole_file: false,
        };
        let mut location = Location {
            path: "src/lib.rs".into(),
            line_start: Some(1),
            line_end: Some(10),
            symbol: None,
            language: None,
            fingerprint: None,
            watch: Some(WatchMode::Range),
        };
        assert!(!anchor_matches(&location, &changed));
        location.watch = Some(WatchMode::File);
        assert!(anchor_matches(&location, &changed));
    }
}
