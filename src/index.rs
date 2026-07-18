use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

use crate::{
    annotations,
    config::Config,
    metadata,
    model::{
        Anchor, Diagnostic, FileRecord, Index, Indexed, IndexedMember, IndexedRelation, LineRange,
        Location, Provenance, Severity, SourceDocument, Strand, WatchMode,
    },
    repo::Repository,
};

pub struct BuildResult {
    pub index: Index,
    pub refreshed: bool,
}

pub fn ensure(repo: &Repository, config: &Config, disable_auto: bool) -> Result<BuildResult> {
    let manifest = manifest(repo, config)?;
    let current_config_fingerprint = config_fingerprint(config)?;
    let path = cache_path(repo, config);
    let cached = read(&path).ok();
    if let Some(index) = cached.as_ref() {
        if index.workspace_fingerprint == manifest.fingerprint
            && index.config_fingerprint == current_config_fingerprint
        {
            return Ok(BuildResult {
                index: index.clone(),
                refreshed: false,
            });
        }
    }
    if disable_auto || !config.index.auto_refresh {
        let reason = cached.as_ref().map_or_else(
            || "missing or incompatible".to_string(),
            |index| {
                if index.workspace_fingerprint != manifest.fingerprint {
                    let changed = manifest.files.iter().find_map(|(path, current)| {
                        let previous = index.files.get(path)?;
                        (previous.size != current.size
                            || previous.modified_ns != current.modified_ns)
                            .then_some(path)
                    });
                    let added = manifest
                        .files
                        .keys()
                        .find(|path| !index.files.contains_key(*path));
                    let removed = index
                        .files
                        .keys()
                        .find(|path| !manifest.files.contains_key(*path));
                    changed.or(added).or(removed).map_or_else(
                        || "repository file manifest changed".to_string(),
                        |path| format!("repository file changed: {path}"),
                    )
                } else if index.config_fingerprint != current_config_fingerprint {
                    "configuration changed".to_string()
                } else {
                    "unknown index mismatch".to_string()
                }
            },
        );
        bail!("Strandmap index is stale ({reason}); run `strandmap index`");
    }
    let index = match cached {
        Some(index)
            if index.config_fingerprint == current_config_fingerprint
                && index.metadata_fingerprint == manifest.metadata_fingerprint =>
        {
            refresh_with_manifest(repo, config, manifest, index)?
        }
        _ => build_with_manifest(repo, config, manifest)?,
    };
    write(repo, config, &index)?;
    Ok(BuildResult {
        index,
        refreshed: true,
    })
}

pub fn rebuild(repo: &Repository, config: &Config) -> Result<Index> {
    let index = build_with_manifest(repo, config, manifest(repo, config)?)?;
    write(repo, config, &index)?;
    Ok(index)
}

pub fn refresh(repo: &Repository, config: &Config) -> Result<BuildResult> {
    let manifest = manifest(repo, config)?;
    let current_config_fingerprint = config_fingerprint(config)?;
    let path = cache_path(repo, config);
    let cached = read(&path).ok();
    if let Some(index) = cached.as_ref() {
        if index.workspace_fingerprint == manifest.fingerprint
            && index.config_fingerprint == current_config_fingerprint
        {
            return Ok(BuildResult {
                index: index.clone(),
                refreshed: false,
            });
        }
    }
    let index = match cached {
        Some(index)
            if index.config_fingerprint == current_config_fingerprint
                && index.metadata_fingerprint == manifest.metadata_fingerprint =>
        {
            refresh_with_manifest(repo, config, manifest, index)?
        }
        _ => build_with_manifest(repo, config, manifest)?,
    };
    write(repo, config, &index)?;
    Ok(BuildResult {
        index,
        refreshed: true,
    })
}

pub fn cache_path(repo: &Repository, config: &Config) -> PathBuf {
    repo.metadata_dir.join(&config.index.path)
}

struct Manifest {
    files: BTreeMap<String, FileRecord>,
    source_paths: Vec<PathBuf>,
    fingerprint: String,
    metadata_fingerprint: String,
}

fn manifest(repo: &Repository, config: &Config) -> Result<Manifest> {
    let include = compile_globs(&config.scan.include, "scan.include")?;
    let exclude = compile_globs(&config.scan.exclude, "scan.exclude")?;
    let metadata_relative = repo.relative(&repo.metadata_dir);
    let mut builder = WalkBuilder::new(&repo.root);
    builder
        .hidden(!config.scan.hidden)
        .follow_links(config.scan.follow_symlinks)
        .git_ignore(config.scan.respect_gitignore)
        .git_global(config.scan.respect_gitignore)
        .git_exclude(config.scan.respect_gitignore);
    let mut files = BTreeMap::new();
    let mut source_paths = Vec::new();
    for entry in builder.build() {
        let entry = entry.with_context(|| format!("failed to scan {}", repo.root.display()))?;
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let path = entry.path();
        let relative = repo.relative(path);
        if relative == metadata_relative || relative.starts_with(&format!("{metadata_relative}/")) {
            continue;
        }
        if (!config.scan.include.is_empty() && !include.is_match(&relative))
            || exclude.is_match(&relative)
        {
            continue;
        }
        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if metadata.len() > config.scan.max_file_bytes {
            continue;
        }
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_nanos().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default();
        files.insert(
            relative.clone(),
            FileRecord {
                path: relative,
                size: metadata.len(),
                modified_ns,
                content_hash: None,
                search_bloom: Vec::new(),
                rust_test_ranges: Vec::new(),
            },
        );
        source_paths.push(path.to_path_buf());
    }
    source_paths.sort();

    let files_fingerprint =
        blake3::hash(&serde_json::to_vec(&files).context("failed to fingerprint files")?);
    let mut metadata_hasher = blake3::Hasher::new();
    for kind in ["strands", "anchors", "relations"] {
        for path in metadata::metadata_paths(repo, kind) {
            metadata_hasher.update(repo.relative(&path).as_bytes());
            metadata_hasher.update(
                &fs::read(&path)
                    .with_context(|| format!("failed to fingerprint {}", path.display()))?,
            );
        }
    }
    for name in ["config.yaml", "config.yml", "config.json", "config.toml"] {
        let path = repo.metadata_dir.join(name);
        if path.is_file() {
            metadata_hasher.update(&fs::read(&path)?);
        }
    }
    let metadata_fingerprint = metadata_hasher.finalize().to_hex().to_string();
    let mut workspace_hasher = blake3::Hasher::new();
    workspace_hasher.update(files_fingerprint.as_bytes());
    workspace_hasher.update(metadata_fingerprint.as_bytes());
    Ok(Manifest {
        files,
        source_paths,
        fingerprint: workspace_hasher.finalize().to_hex().to_string(),
        metadata_fingerprint,
    })
}

pub(crate) fn source_paths(repo: &Repository, config: &Config) -> Result<Vec<PathBuf>> {
    Ok(manifest(repo, config)?.source_paths)
}

fn build_with_manifest(
    repo: &Repository,
    config: &Config,
    mut manifest: Manifest,
) -> Result<Index> {
    let mut source_documents = BTreeMap::new();
    for path in &manifest.source_paths {
        let relative = repo.relative(path);
        let document = scan_source_document(
            path,
            &relative,
            config,
            manifest
                .files
                .get_mut(&relative)
                .expect("manifest source has a file record"),
        );
        source_documents.insert(relative, document);
    }
    finish_index(repo, config, manifest, source_documents)
}

fn refresh_with_manifest(
    repo: &Repository,
    config: &Config,
    mut manifest: Manifest,
    mut cached: Index,
) -> Result<Index> {
    let mut changed: BTreeSet<String> = cached
        .files
        .keys()
        .filter(|path| !manifest.files.contains_key(*path))
        .cloned()
        .collect();
    for (relative, record) in &manifest.files {
        if !cached.files.get(relative).is_some_and(|previous| {
            previous.size == record.size && previous.modified_ns == record.modified_ns
        }) {
            changed.insert(relative.clone());
        }
    }

    let mut affected_strands = BTreeSet::new();
    let mut affected_anchors = BTreeSet::new();
    for path in &changed {
        if let Some(document) = cached.source_documents.get(path) {
            collect_document_impacts(document, &mut affected_strands, &mut affected_anchors);
        }
    }
    cached
        .source_documents
        .retain(|path, _| manifest.files.contains_key(path));

    for (relative, record) in &mut manifest.files {
        if !changed.contains(relative) {
            *record = cached
                .files
                .get(relative)
                .expect("unchanged source has a cached record")
                .clone();
            continue;
        }
        let document = scan_source_document(&repo.root.join(relative), relative, config, record);
        collect_document_impacts(&document, &mut affected_strands, &mut affected_anchors);
        cached.source_documents.insert(relative.clone(), document);
    }

    refresh_graph(
        repo,
        &mut cached,
        &changed,
        &mut affected_strands,
        &mut affected_anchors,
    );
    cached.generated_at = Utc::now();
    cached.workspace_fingerprint = manifest.fingerprint;
    cached.metadata_fingerprint = manifest.metadata_fingerprint;
    cached.config_fingerprint = config_fingerprint(config)?;
    cached.files = manifest.files;
    Ok(cached)
}

fn collect_document_impacts(
    document: &SourceDocument,
    strands: &mut BTreeSet<String>,
    anchors: &mut BTreeSet<String>,
) {
    strands.extend(
        document
            .strands
            .iter()
            .map(|indexed| indexed.value.id.clone()),
    );
    anchors.extend(
        document
            .anchors
            .iter()
            .map(|indexed| indexed.value.id.clone()),
    );
    for membership in &document.memberships {
        if let Some(strand) = &membership.strand {
            strands.insert(strand.clone());
        }
        anchors.insert(membership.member.anchor.clone());
    }
    for relation in &document.relations {
        anchors.insert(relation.relation.from.clone());
        anchors.insert(relation.relation.to.clone());
    }
}

fn refresh_graph(
    repo: &Repository,
    index: &mut Index,
    changed: &BTreeSet<String>,
    affected_strands: &mut BTreeSet<String>,
    affected_anchors: &mut BTreeSet<String>,
) {
    let changed_annotation = |provenance: &Provenance| {
        provenance.source == "annotation"
            && provenance
                .path
                .as_ref()
                .is_some_and(|path| changed.contains(path))
    };
    index
        .memberships
        .retain(|membership| !changed_annotation(&membership.provenance));
    index
        .relations
        .retain(|relation| !changed_annotation(&relation.provenance));
    for path in changed {
        if let Some(document) = index.source_documents.get(path) {
            index
                .memberships
                .extend(document.memberships.iter().cloned());
            index.relations.extend(document.relations.iter().cloned());
        }
    }
    sort_edges(&mut index.memberships, &mut index.relations);
    deduplicate_edges(&mut index.memberships, &mut index.relations);

    let loaded = metadata::load(repo);
    for strand_id in affected_strands.iter() {
        let sidecar = loaded
            .strands
            .iter()
            .find(|item| item.value.id == *strand_id)
            .map(|item| Indexed {
                value: item.value.clone(),
                provenance: Provenance {
                    source: "sidecar".into(),
                    path: Some(repo.relative(&item.path)),
                    line: item.line,
                },
            });
        let source = index.source_documents.values().find_map(|document| {
            document
                .strands
                .iter()
                .find(|indexed| indexed.value.id == *strand_id)
                .cloned()
        });
        let implicit = index
            .memberships
            .iter()
            .find(|membership| membership.strand.as_deref() == Some(strand_id))
            .map(|membership| Indexed {
                value: Strand {
                    schema: 1,
                    id: strand_id.clone(),
                    title: None,
                    intent: String::new(),
                    scope: None,
                    tags: BTreeSet::new(),
                    members: Vec::new(),
                    relations: Vec::new(),
                    on_change: None,
                    attributes: BTreeMap::new(),
                },
                provenance: membership.provenance.clone(),
            });
        if let Some(strand) = sidecar.or(source).or(implicit) {
            index.strands.insert(strand_id.clone(), strand);
        } else {
            index.strands.remove(strand_id);
        }
    }

    for anchor_id in affected_anchors.iter() {
        let mut selected = loaded
            .anchors
            .iter()
            .find(|item| item.value.id == *anchor_id)
            .map(|item| {
                let mut value = item.value.clone();
                if let Some(location) = &mut value.location {
                    location.path = normalize_repository_path(&location.path);
                }
                Indexed {
                    value,
                    provenance: Provenance {
                        source: "sidecar".into(),
                        path: Some(repo.relative(&item.path)),
                        line: item.line,
                    },
                }
            });
        for source in index.source_documents.values().flat_map(|document| {
            document
                .anchors
                .iter()
                .filter(|indexed| indexed.value.id == *anchor_id)
        }) {
            if let Some(existing) = &mut selected {
                if existing.provenance.source == "sidecar" {
                    merge_source_anchor(&mut existing.value, &source.value);
                }
            } else {
                selected = Some(source.clone());
            }
        }
        if selected.is_none() && looks_like_target(anchor_id) {
            let provenance = index
                .memberships
                .iter()
                .find(|membership| membership.member.anchor == *anchor_id)
                .map(|membership| membership.provenance.clone())
                .or_else(|| {
                    index.relations.iter().find_map(|relation| {
                        (relation.relation.from == *anchor_id || relation.relation.to == *anchor_id)
                            .then(|| relation.provenance.clone())
                    })
                });
            selected = provenance.map(|provenance| Indexed {
                value: Anchor {
                    schema: 1,
                    id: anchor_id.clone(),
                    target: Some(anchor_id.clone()),
                    kind: None,
                    location: parse_file_target(anchor_id),
                    tags: BTreeSet::new(),
                    attributes: BTreeMap::new(),
                },
                provenance,
            });
        }
        if let Some(mut anchor) = selected {
            if anchor.value.location.is_none() {
                if let Some(location) = anchor.value.target.as_deref().and_then(parse_file_target) {
                    anchor.value.location = Some(location);
                }
            }
            index.anchors.insert(anchor_id.clone(), anchor);
        } else {
            index.anchors.remove(anchor_id);
        }
    }

    index.diagnostics = recompute_diagnostics(repo, &loaded, index);
}

fn sort_edges(memberships: &mut [IndexedMember], relations: &mut [IndexedRelation]) {
    memberships.sort_by(|left, right| {
        (left.provenance.source != "sidecar")
            .cmp(&(right.provenance.source != "sidecar"))
            .then(left.provenance.path.cmp(&right.provenance.path))
            .then(left.provenance.line.cmp(&right.provenance.line))
            .then(left.strand.cmp(&right.strand))
            .then(left.member.anchor.cmp(&right.member.anchor))
            .then(left.member.role.cmp(&right.member.role))
    });
    relations.sort_by(|left, right| {
        (left.provenance.source != "sidecar")
            .cmp(&(right.provenance.source != "sidecar"))
            .then(left.provenance.path.cmp(&right.provenance.path))
            .then(left.provenance.line.cmp(&right.provenance.line))
            .then(left.strand.cmp(&right.strand))
            .then(left.relation.from.cmp(&right.relation.from))
            .then(left.relation.to.cmp(&right.relation.to))
            .then(left.relation.kind.cmp(&right.relation.kind))
    });
}

fn recompute_diagnostics<'a>(
    repo: &Repository,
    loaded: &'a metadata::LoadedMetadata,
    index: &'a Index,
) -> Vec<Diagnostic> {
    enum SeenAnchor<'a> {
        Sidecar {
            target: Option<String>,
            provenance: Provenance,
        },
        Annotation(&'a Indexed<Anchor>),
    }

    let mut diagnostics = loaded.diagnostics.clone();
    let mut seen_strands: BTreeMap<&str, Provenance> = BTreeMap::new();
    for item in &loaded.strands {
        let provenance = Provenance {
            source: "sidecar".into(),
            path: Some(repo.relative(&item.path)),
            line: item.line,
        };
        validate_strand(&item.value, &provenance, &mut diagnostics);
        if let Some(original) = seen_strands.get(item.value.id.as_str()) {
            diagnostics.push(duplicate(
                "duplicate-strand",
                &item.value.id,
                &provenance,
                original,
            ));
        } else {
            seen_strands.insert(&item.value.id, provenance);
        }
    }

    let mut seen_anchors: BTreeMap<&str, SeenAnchor<'_>> = BTreeMap::new();
    for item in &loaded.anchors {
        let provenance = Provenance {
            source: "sidecar".into(),
            path: Some(repo.relative(&item.path)),
            line: item.line,
        };
        let mut anchor = item.value.clone();
        if let Some(location) = &mut anchor.location {
            location.path = normalize_repository_path(&location.path);
        }
        validate_anchor(&anchor, &provenance, &mut diagnostics);
        if let Some(original) = seen_anchors.get(item.value.id.as_str()) {
            let original = match original {
                SeenAnchor::Sidecar { provenance, .. } => provenance,
                SeenAnchor::Annotation(indexed) => &indexed.provenance,
            };
            diagnostics.push(duplicate(
                "duplicate-anchor",
                &item.value.id,
                &provenance,
                original,
            ));
        } else {
            seen_anchors.insert(
                &item.value.id,
                SeenAnchor::Sidecar {
                    target: item.value.target.clone(),
                    provenance,
                },
            );
        }
    }

    for document in index.source_documents.values() {
        diagnostics.extend(document.diagnostics.iter().cloned());
        for indexed in &document.strands {
            if let Some(original) = seen_strands.get(indexed.value.id.as_str()) {
                if original.source == "annotation" {
                    diagnostics.push(duplicate(
                        "duplicate-strand",
                        &indexed.value.id,
                        &indexed.provenance,
                        original,
                    ));
                }
            } else {
                seen_strands.insert(&indexed.value.id, indexed.provenance.clone());
            }
        }
        for indexed in &document.anchors {
            match seen_anchors.get_mut(indexed.value.id.as_str()) {
                Some(SeenAnchor::Sidecar {
                    target,
                    provenance: _,
                }) => {
                    if target.is_some()
                        && indexed.value.target.is_some()
                        && *target != indexed.value.target
                    {
                        diagnostics.push(at(
                            Severity::Error,
                            "anchor-target-conflict",
                            format!(
                                "anchor {:?} has conflicting sidecar and source targets",
                                indexed.value.id
                            ),
                            &indexed.provenance,
                        ));
                    }
                    if target.is_none() {
                        target.clone_from(&indexed.value.target);
                    }
                }
                Some(SeenAnchor::Annotation(original)) => {
                    if original.value.location != indexed.value.location
                        || original.value.target != indexed.value.target
                    {
                        diagnostics.push(duplicate(
                            "duplicate-anchor",
                            &indexed.value.id,
                            &indexed.provenance,
                            &original.provenance,
                        ));
                    }
                }
                None => {
                    seen_anchors.insert(&indexed.value.id, SeenAnchor::Annotation(indexed));
                }
            }
        }
    }

    let mut implicit_strands = BTreeSet::new();
    for membership in &index.memberships {
        let Some(strand_id) = membership.strand.as_deref() else {
            continue;
        };
        if !seen_strands.contains_key(strand_id) && implicit_strands.insert(strand_id) {
            diagnostics.push(at(
                Severity::Warning,
                "implicit-strand",
                format!("strand {strand_id:?} is declared only by membership annotations"),
                &membership.provenance,
            ));
        }
    }
    validate_graph(
        &index.strands,
        &index.anchors,
        &index.memberships,
        &index.relations,
        &mut diagnostics,
    );
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.line.cmp(&right.line))
            .then(left.code.cmp(&right.code))
    });
    diagnostics
}

fn scan_source_document(
    path: &Path,
    relative: &str,
    config: &Config,
    record: &mut FileRecord,
) -> SourceDocument {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return SourceDocument {
                diagnostics: vec![Diagnostic {
                    severity: Severity::Warning,
                    code: "source-read".into(),
                    message: error.to_string(),
                    path: Some(relative.into()),
                    line: None,
                    hint: None,
                }],
                ..SourceDocument::default()
            };
        }
    };
    record.content_hash = Some(blake3::hash(&bytes).to_hex().to_string());
    if bytes.contains(&0) {
        return SourceDocument::default();
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return SourceDocument {
            diagnostics: vec![Diagnostic {
                severity: Severity::Info,
                code: "source-encoding".into(),
                message: "source is not UTF-8; annotations were not scanned".into(),
                path: Some(relative.into()),
                line: None,
                hint: None,
            }],
            ..SourceDocument::default()
        };
    };
    record.search_bloom = crate::search::source_bloom(text);
    record.rust_test_ranges = crate::source_span::rust_test_ranges(relative, text)
        .into_iter()
        .map(|span| LineRange {
            start: span.start_line,
            end: span.end_line,
        })
        .collect();
    let scan = annotations::scan_source(relative, text, &config.annotations);
    SourceDocument {
        anchors: scan
            .anchors
            .into_iter()
            .map(|(value, provenance)| Indexed { value, provenance })
            .collect(),
        strands: scan
            .strands
            .into_iter()
            .map(|(value, provenance)| Indexed { value, provenance })
            .collect(),
        memberships: scan.memberships,
        relations: scan.relations,
        diagnostics: scan.diagnostics,
    }
}

fn finish_index(
    repo: &Repository,
    config: &Config,
    manifest: Manifest,
    source_documents: BTreeMap<String, SourceDocument>,
) -> Result<Index> {
    let (strands, anchors, memberships, relations, diagnostics) =
        assemble_graph(repo, &source_documents);
    Ok(Index {
        schema: 5,
        root: repo.root.to_string_lossy().into_owned(),
        generated_at: Utc::now(),
        workspace_fingerprint: manifest.fingerprint,
        metadata_fingerprint: manifest.metadata_fingerprint,
        config_fingerprint: config_fingerprint(config)?,
        files: manifest.files,
        source_documents,
        strands,
        anchors,
        memberships,
        relations,
        diagnostics,
    })
}

type Graph = (
    BTreeMap<String, Indexed<Strand>>,
    BTreeMap<String, Indexed<Anchor>>,
    Vec<IndexedMember>,
    Vec<IndexedRelation>,
    Vec<Diagnostic>,
);

fn assemble_graph(repo: &Repository, source_documents: &BTreeMap<String, SourceDocument>) -> Graph {
    let loaded = metadata::load(repo);
    let mut diagnostics = loaded.diagnostics;
    let mut strands: BTreeMap<String, Indexed<Strand>> = BTreeMap::new();
    let mut anchors: BTreeMap<String, Indexed<Anchor>> = BTreeMap::new();
    let mut memberships = Vec::new();
    let mut relations = Vec::new();

    for item in loaded.strands {
        let provenance = Provenance {
            source: "sidecar".into(),
            path: Some(repo.relative(&item.path)),
            line: item.line,
        };
        validate_strand(&item.value, &provenance, &mut diagnostics);
        for member in &item.value.members {
            memberships.push(IndexedMember {
                strand: Some(item.value.id.clone()),
                member: member.clone(),
                provenance: provenance.clone(),
            });
        }
        for relation in &item.value.relations {
            relations.push(IndexedRelation {
                strand: Some(item.value.id.clone()),
                relation: relation.clone(),
                provenance: provenance.clone(),
            });
        }
        if let Some(existing) = strands.get(&item.value.id) {
            diagnostics.push(duplicate(
                "duplicate-strand",
                &item.value.id,
                &provenance,
                &existing.provenance,
            ));
        } else {
            strands.insert(
                item.value.id.clone(),
                Indexed {
                    value: item.value,
                    provenance,
                },
            );
        }
    }

    for item in loaded.anchors {
        let mut anchor = item.value;
        if let Some(location) = &mut anchor.location {
            location.path = normalize_repository_path(&location.path);
        }
        let provenance = Provenance {
            source: "sidecar".into(),
            path: Some(repo.relative(&item.path)),
            line: item.line,
        };
        validate_anchor(&anchor, &provenance, &mut diagnostics);
        if let Some(existing) = anchors.get(&anchor.id) {
            diagnostics.push(duplicate(
                "duplicate-anchor",
                &anchor.id,
                &provenance,
                &existing.provenance,
            ));
        } else {
            anchors.insert(
                anchor.id.clone(),
                Indexed {
                    value: anchor,
                    provenance,
                },
            );
        }
    }

    for item in loaded.relations {
        relations.push(IndexedRelation {
            strand: None,
            relation: item.value,
            provenance: Provenance {
                source: "sidecar".into(),
                path: Some(repo.relative(&item.path)),
                line: item.line,
            },
        });
    }

    for document in source_documents.values() {
        diagnostics.extend(document.diagnostics.iter().cloned());
        memberships.extend(document.memberships.iter().cloned());
        relations.extend(document.relations.iter().cloned());
        for indexed in &document.strands {
            let strand = &indexed.value;
            let provenance = &indexed.provenance;
            match strands.get(&strand.id) {
                Some(existing) if existing.provenance.source == "annotation" => {
                    diagnostics.push(duplicate(
                        "duplicate-strand",
                        &strand.id,
                        provenance,
                        &existing.provenance,
                    ));
                }
                Some(_) => {}
                None => {
                    strands.insert(
                        strand.id.clone(),
                        Indexed {
                            value: strand.clone(),
                            provenance: provenance.clone(),
                        },
                    );
                }
            }
        }
        for indexed in &document.anchors {
            let mut anchor = indexed.value.clone();
            let provenance = &indexed.provenance;
            if let Some(location) = &mut anchor.location {
                location.path = normalize_repository_path(&location.path);
            }
            if let Some(existing) = anchors.get_mut(&anchor.id) {
                if existing.provenance.source == "sidecar" {
                    if existing.value.target.is_some()
                        && anchor.target.is_some()
                        && existing.value.target != anchor.target
                    {
                        diagnostics.push(at(
                            Severity::Error,
                            "anchor-target-conflict",
                            format!(
                                "anchor {:?} has conflicting sidecar and source targets",
                                anchor.id
                            ),
                            provenance,
                        ));
                    }
                    merge_source_anchor(&mut existing.value, &anchor);
                } else if existing.value.location != anchor.location
                    || existing.value.target != anchor.target
                {
                    diagnostics.push(duplicate(
                        "duplicate-anchor",
                        &anchor.id,
                        provenance,
                        &existing.provenance,
                    ));
                }
            } else {
                anchors.insert(
                    anchor.id.clone(),
                    Indexed {
                        value: anchor,
                        provenance: provenance.clone(),
                    },
                );
            }
        }
    }

    materialize_targets(&mut anchors);
    materialize_missing_strands(&mut strands, &memberships, &mut diagnostics);
    materialize_uri_anchors(&mut anchors, &memberships, &relations);
    validate_graph(
        &strands,
        &anchors,
        &memberships,
        &relations,
        &mut diagnostics,
    );
    deduplicate_edges(&mut memberships, &mut relations);
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.line.cmp(&right.line))
            .then(left.code.cmp(&right.code))
    });

    (strands, anchors, memberships, relations, diagnostics)
}

fn config_fingerprint(config: &Config) -> Result<String> {
    Ok(blake3::hash(&serde_json::to_vec(config)?)
        .to_hex()
        .to_string())
}

fn read(path: &Path) -> Result<Index> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let index: Index = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid index at {}", path.display()))?;
    if index.schema != 5 {
        bail!("unsupported index schema {}", index.schema);
    }
    Ok(index)
}

fn write(repo: &Repository, config: &Config, index: &Index) -> Result<()> {
    let path = cache_path(repo, config);
    let parent = path.parent().context("index path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "failed to create index temporary file in {}",
            parent.display()
        )
    })?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        serde_json::to_writer(&mut writer, index).context("failed to encode index")?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    temporary
        .persist(&path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

fn compile_globs(patterns: &[String], field: &str) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            Glob::new(pattern).with_context(|| format!("invalid glob in {field}: {pattern:?}"))?,
        );
    }
    builder.build().context("failed to compile glob set")
}

fn validate_strand(strand: &Strand, provenance: &Provenance, diagnostics: &mut Vec<Diagnostic>) {
    if strand.schema != 1 {
        diagnostics.push(at(
            Severity::Error,
            "strand-schema",
            format!(
                "strand {:?} uses unsupported schema {}",
                strand.id, strand.schema
            ),
            provenance,
        ));
    }
    if strand.id.trim().is_empty() || strand.id.chars().any(char::is_whitespace) {
        diagnostics.push(at(
            Severity::Error,
            "strand-id",
            "strand id cannot be empty or contain whitespace".into(),
            provenance,
        ));
    }
    if strand.intent.trim().is_empty() {
        diagnostics.push(at(
            Severity::Warning,
            "strand-intent",
            format!("strand {:?} has no intent", strand.id),
            provenance,
        ));
    }
}

fn validate_anchor(anchor: &Anchor, provenance: &Provenance, diagnostics: &mut Vec<Diagnostic>) {
    if anchor.schema != 1 {
        diagnostics.push(at(
            Severity::Error,
            "anchor-schema",
            format!(
                "anchor {:?} uses unsupported schema {}",
                anchor.id, anchor.schema
            ),
            provenance,
        ));
    }
    if anchor.id.trim().is_empty() || anchor.id.chars().any(char::is_whitespace) {
        diagnostics.push(at(
            Severity::Error,
            "anchor-id",
            "anchor id cannot be empty or contain whitespace".into(),
            provenance,
        ));
    }
    reject_removed_attributes(
        &anchor.attributes,
        &["description"],
        "anchor",
        &anchor.id,
        provenance,
        diagnostics,
    );
    if anchor.target.is_none() && anchor.location.is_none() {
        diagnostics.push(at(
            Severity::Warning,
            "anchor-unlocated",
            format!("anchor {:?} has neither target nor location", anchor.id),
            provenance,
        ));
    }
    if let Some(location) = &anchor.location {
        if location.watch == Some(WatchMode::Node) && provenance.source != "annotation" {
            diagnostics.push(at(
                Severity::Error,
                "anchor-node-location",
                format!(
                    "anchor {:?} uses node watch outside a source annotation",
                    anchor.id
                ),
                provenance,
            ));
        }
        if location.path.trim().is_empty()
            || location.path.starts_with('/')
            || location.path.split('/').any(|part| part == "..")
        {
            diagnostics.push(at(
                Severity::Error,
                "anchor-path",
                format!("anchor {:?} has a non-repository-relative path", anchor.id),
                provenance,
            ));
        }
        if location.line_start.is_some_and(|line| line == 0)
            || location.line_end.is_some_and(|line| line == 0)
            || location
                .line_start
                .zip(location.line_end)
                .is_some_and(|(start, end)| end < start)
        {
            diagnostics.push(at(
                Severity::Error,
                "anchor-range",
                format!("anchor {:?} has an invalid line range", anchor.id),
                provenance,
            ));
        }
    }
}

fn merge_source_anchor(sidecar: &mut Anchor, source: &Anchor) {
    sidecar.location = source.location.clone().or_else(|| sidecar.location.clone());
    if sidecar.target.is_none() {
        sidecar.target.clone_from(&source.target);
    }
    if sidecar.kind.is_none() {
        sidecar.kind.clone_from(&source.kind);
    }
    sidecar.tags.extend(source.tags.iter().cloned());
}

fn materialize_targets(anchors: &mut BTreeMap<String, Indexed<Anchor>>) {
    for indexed in anchors.values_mut() {
        if indexed.value.location.is_none() {
            if let Some(target) = indexed.value.target.as_deref().and_then(parse_file_target) {
                indexed.value.location = Some(target);
            }
        }
    }
}

fn parse_file_target(target: &str) -> Option<Location> {
    let target = target.strip_prefix("file://").unwrap_or(target);
    if target.contains("://") {
        return None;
    }
    let (path, fragment) = target
        .split_once('#')
        .map_or((target, None), |(path, fragment)| (path, Some(fragment)));
    if path.is_empty() {
        return None;
    }
    let (line_start, line_end) = fragment
        .and_then(|fragment| fragment.strip_prefix('L'))
        .and_then(|range| {
            let (start, end) = range.split_once('-').map_or((range, range), |parts| parts);
            Some((
                start.parse::<u32>().ok()?,
                end.trim_start_matches('L').parse::<u32>().ok()?,
            ))
        })
        .map_or((None, None), |(start, end)| (Some(start), Some(end)));
    Some(Location {
        path: normalize_repository_path(path),
        line_start,
        line_end,
        symbol: None,
        language: None,
        fingerprint: None,
        watch: Some(if line_start.is_some() {
            WatchMode::Range
        } else {
            WatchMode::File
        }),
    })
}

fn normalize_repository_path(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    normalized
}

fn materialize_missing_strands(
    strands: &mut BTreeMap<String, Indexed<Strand>>,
    memberships: &[IndexedMember],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for membership in memberships {
        let Some(strand_id) = &membership.strand else {
            continue;
        };
        if !strands.contains_key(strand_id) {
            diagnostics.push(at(
                Severity::Warning,
                "implicit-strand",
                format!("strand {strand_id:?} is declared only by membership annotations"),
                &membership.provenance,
            ));
            strands.insert(
                strand_id.clone(),
                Indexed {
                    value: Strand {
                        schema: 1,
                        id: strand_id.clone(),
                        title: None,
                        intent: String::new(),
                        scope: None,
                        tags: BTreeSet::new(),
                        members: Vec::new(),
                        relations: Vec::new(),
                        on_change: None,
                        attributes: BTreeMap::new(),
                    },
                    provenance: membership.provenance.clone(),
                },
            );
        }
    }
}

fn materialize_uri_anchors(
    anchors: &mut BTreeMap<String, Indexed<Anchor>>,
    memberships: &[IndexedMember],
    relations: &[IndexedRelation],
) {
    let mut references = Vec::new();
    for membership in memberships {
        references.push((
            membership.member.anchor.clone(),
            membership.provenance.clone(),
        ));
    }
    for relation in relations {
        references.push((relation.relation.from.clone(), relation.provenance.clone()));
        references.push((relation.relation.to.clone(), relation.provenance.clone()));
    }
    for (id, provenance) in references {
        if anchors.contains_key(&id) || !looks_like_target(&id) {
            continue;
        }
        anchors.insert(
            id.clone(),
            Indexed {
                value: Anchor {
                    schema: 1,
                    id: id.clone(),
                    target: Some(id.clone()),
                    kind: None,
                    location: parse_file_target(&id),
                    tags: BTreeSet::new(),
                    attributes: BTreeMap::new(),
                },
                provenance,
            },
        );
    }
}

fn looks_like_target(value: &str) -> bool {
    value.contains("://") || value.contains("#L") || value.contains('/')
}

fn validate_graph(
    strands: &BTreeMap<String, Indexed<Strand>>,
    anchors: &BTreeMap<String, Indexed<Anchor>>,
    memberships: &[IndexedMember],
    relations: &[IndexedRelation],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for membership in memberships {
        reject_removed_attributes(
            &membership.member.attributes,
            &["rationale", "reason"],
            "member",
            &membership.member.anchor,
            &membership.provenance,
            diagnostics,
        );
        if !anchors.contains_key(&membership.member.anchor) {
            diagnostics.push(at(
                Severity::Error,
                "unresolved-anchor",
                format!("unknown anchor {:?}", membership.member.anchor),
                &membership.provenance,
            ));
        }
        if let Some(strand) = &membership.strand {
            if !strands.contains_key(strand) {
                diagnostics.push(at(
                    Severity::Error,
                    "unresolved-strand",
                    format!("unknown strand {strand:?}"),
                    &membership.provenance,
                ));
            }
        }
    }
    for relation in relations {
        reject_removed_attributes(
            &relation.relation.attributes,
            &["rationale", "reason"],
            "relation",
            &relation.relation.kind,
            &relation.provenance,
            diagnostics,
        );
        for anchor in [&relation.relation.from, &relation.relation.to] {
            if !anchors.contains_key(anchor) {
                diagnostics.push(at(
                    Severity::Error,
                    "unresolved-relation-anchor",
                    format!("relationship references unknown anchor {anchor:?}"),
                    &relation.provenance,
                ));
            }
        }
        if relation.relation.kind.trim().is_empty() {
            diagnostics.push(at(
                Severity::Error,
                "relation-kind",
                "relationship type cannot be empty".into(),
                &relation.provenance,
            ));
        }
    }
}

fn reject_removed_attributes(
    attributes: &crate::model::Attributes,
    names: &[&str],
    kind: &str,
    id: &str,
    provenance: &Provenance,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for name in names {
        if attributes.contains_key(*name) {
            diagnostics.push(at(
                Severity::Error,
                "removed-metadata-field",
                format!("{kind} {id:?} uses unsupported {name} metadata"),
                provenance,
            ));
        }
    }
}

fn deduplicate_edges(memberships: &mut Vec<IndexedMember>, relations: &mut Vec<IndexedRelation>) {
    let mut seen_members = BTreeSet::new();
    memberships.retain(|membership| {
        let key = (
            membership.strand.clone(),
            membership.member.anchor.clone(),
            membership.member.role.clone(),
        );
        seen_members.insert(key)
    });
    let mut seen_relations = BTreeSet::new();
    relations.retain(|relation| {
        let key = (
            relation.strand.clone(),
            relation.relation.from.clone(),
            relation.relation.to.clone(),
            relation.relation.kind.clone(),
        );
        seen_relations.insert(key)
    });
}

fn duplicate(code: &str, id: &str, current: &Provenance, original: &Provenance) -> Diagnostic {
    let original_location = original.path.as_deref().unwrap_or(&original.source);
    let mut diagnostic = at(
        Severity::Error,
        code,
        format!("duplicate id {id:?}; first defined at {original_location}"),
        current,
    );
    diagnostic.hint = Some(
        "Use one definition per id; sidecar anchors may be located by a matching source annotation"
            .into(),
    );
    diagnostic
}

fn at(severity: Severity, code: &str, message: String, provenance: &Provenance) -> Diagnostic {
    Diagnostic {
        severity,
        code: code.into(),
        message,
        path: provenance.path.clone(),
        line: provenance.line,
        hint: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn file_target_parses_ranges() {
        let location = parse_file_target("file://src/auth.rs#L10-L20").unwrap();
        assert_eq!(location.path, "src/auth.rs");
        assert_eq!(location.line_start, Some(10));
        assert_eq!(location.line_end, Some(20));
    }

    #[test]
    fn incremental_refresh_matches_a_clean_rebuild() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        fs::create_dir_all(root.join(".strandmap")).unwrap();
        fs::create_dir_all(root.join(".strandmap/anchors")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        let config = Config::default();
        fs::write(
            root.join(".strandmap/config.yaml"),
            serde_yaml_ng::to_string(&config).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join(".strandmap/anchors/alpha.yaml"),
            serde_yaml_ng::to_string(&Anchor {
                schema: 1,
                id: "alpha".into(),
                target: None,
                kind: Some("function".into()),
                location: None,
                tags: BTreeSet::new(),
                attributes: BTreeMap::new(),
            })
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("src/a.rs"),
            "// @anchor alpha\n// @strand first role=implementation intent=stable\nfn alpha() {}\n",
        )
        .unwrap();
        fs::write(
            root.join("src/b.rs"),
            "// @anchor beta\n// @strand second role=implementation intent=stable\nfn beta() {}\n",
        )
        .unwrap();
        let repo = Repository::discover(Some(root), ".strandmap").unwrap();
        rebuild(&repo, &config).unwrap();

        fs::write(
            root.join("src/a.rs"),
            "// @anchor gamma\n// @strand first role=producer intent=stable\nfn gamma(value: usize) -> usize { value + 1 }\n",
        )
        .unwrap();
        let incremental = refresh(&repo, &config).unwrap().index;
        let rebuilt =
            build_with_manifest(&repo, &config, manifest(&repo, &config).unwrap()).unwrap();

        assert_eq!(incremental.files, rebuilt.files);
        assert_eq!(incremental.source_documents, rebuilt.source_documents);
        assert_eq!(incremental.strands, rebuilt.strands);
        assert_eq!(incremental.anchors, rebuilt.anchors);
        assert_eq!(incremental.memberships, rebuilt.memberships);
        assert_eq!(incremental.relations, rebuilt.relations);
        assert_eq!(incremental.diagnostics, rebuilt.diagnostics);
    }
}
