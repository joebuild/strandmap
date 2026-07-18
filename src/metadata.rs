use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    model::{Anchor, Diagnostic, Relation, Severity, Strand},
    repo::Repository,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum StrandDocument {
    Wrapped { strands: Vec<Strand> },
    List(Vec<Strand>),
    Single(Box<Strand>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum AnchorDocument {
    Wrapped { anchors: Vec<Anchor> },
    List(Vec<Anchor>),
    Single(Box<Anchor>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RelationDocument {
    Wrapped { relations: Vec<Relation> },
    List(Vec<Relation>),
    Single(Box<Relation>),
}

#[derive(Debug, Clone)]
pub struct MetadataItem<T> {
    pub value: T,
    pub path: PathBuf,
    pub line: Option<u32>,
}

#[derive(Debug, Default)]
pub struct LoadedMetadata {
    pub strands: Vec<MetadataItem<Strand>>,
    pub anchors: Vec<MetadataItem<Anchor>>,
    pub relations: Vec<MetadataItem<Relation>>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Default)]
pub struct ParsedMetadata {
    pub strands: Vec<Strand>,
    pub anchors: Vec<Anchor>,
    pub relations: Vec<Relation>,
}

pub fn parse_metadata_text(path: &Path, text: &str) -> Result<ParsedMetadata> {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect();
    let stem = path.file_stem().and_then(std::ffi::OsStr::to_str);
    if components.contains(&"strands") || stem == Some("strands") {
        let document: StrandDocument = parse_document_text(path, text)?;
        return Ok(ParsedMetadata {
            strands: strand_items(&document).into_iter().cloned().collect(),
            ..ParsedMetadata::default()
        });
    }
    if components.contains(&"anchors") || stem == Some("anchors") {
        let document: AnchorDocument = parse_document_text(path, text)?;
        return Ok(ParsedMetadata {
            anchors: anchor_items(&document).into_iter().cloned().collect(),
            ..ParsedMetadata::default()
        });
    }
    if components.contains(&"relations") || stem == Some("relations") {
        let document: RelationDocument = parse_document_text(path, text)?;
        return Ok(ParsedMetadata {
            relations: relation_items(&document).into_iter().cloned().collect(),
            ..ParsedMetadata::default()
        });
    }
    bail!(
        "path is not recognized Strandmap metadata: {}",
        path.display()
    )
}

pub fn load(repo: &Repository) -> LoadedMetadata {
    let mut loaded = LoadedMetadata::default();
    for path in metadata_paths(repo, "strands") {
        match read_document::<StrandDocument>(&path) {
            Ok(document) => {
                for strand in strand_items(&document) {
                    let line = find_id_line(&path, &strand.id);
                    loaded.strands.push(MetadataItem {
                        value: strand.clone(),
                        path: path.clone(),
                        line,
                    });
                }
            }
            Err(error) => loaded
                .diagnostics
                .push(parse_diagnostic(repo, &path, &error)),
        }
    }
    for path in metadata_paths(repo, "anchors") {
        match read_document::<AnchorDocument>(&path) {
            Ok(document) => {
                for anchor in anchor_items(&document) {
                    let line = find_id_line(&path, &anchor.id);
                    loaded.anchors.push(MetadataItem {
                        value: anchor.clone(),
                        path: path.clone(),
                        line,
                    });
                }
            }
            Err(error) => loaded
                .diagnostics
                .push(parse_diagnostic(repo, &path, &error)),
        }
    }
    for path in metadata_paths(repo, "relations") {
        match read_document::<RelationDocument>(&path) {
            Ok(document) => {
                for relation in relation_items(&document) {
                    loaded.relations.push(MetadataItem {
                        value: relation.clone(),
                        path: path.clone(),
                        line: None,
                    });
                }
            }
            Err(error) => loaded
                .diagnostics
                .push(parse_diagnostic(repo, &path, &error)),
        }
    }
    loaded
}

pub fn add_strand(repo: &Repository, strand: &Strand) -> Result<PathBuf> {
    ensure_unique_strand(repo, &strand.id)?;
    let directory = repo.ensure_dir("strands")?;
    let path = unique_path(&directory, &strand.id);
    write_document(&path, &StrandDocument::Single(Box::new(strand.clone())))?;
    Ok(path)
}

pub fn add_anchor(repo: &Repository, anchor: &Anchor) -> Result<PathBuf> {
    ensure_unique_anchor(repo, &anchor.id)?;
    let directory = repo.ensure_dir("anchors")?;
    let path = unique_path(&directory, &anchor.id);
    write_document(&path, &AnchorDocument::Single(Box::new(anchor.clone())))?;
    Ok(path)
}

pub fn add_global_relation(repo: &Repository, relation: &Relation) -> Result<PathBuf> {
    let loaded = load(repo);
    if loaded.relations.iter().any(|item| {
        item.value.from == relation.from
            && item.value.to == relation.to
            && item.value.kind == relation.kind
    }) {
        bail!("that global relationship already exists");
    }
    let directory = repo.ensure_dir("relations")?;
    let identity = format!("{}-{}-{}", relation.from, relation.kind, relation.to);
    let path = unique_path(&directory, &identity);
    write_document(&path, &RelationDocument::Single(Box::new(relation.clone())))?;
    Ok(path)
}

pub fn remove_global_relation(
    repo: &Repository,
    from: &str,
    to: &str,
    kind: &str,
) -> Result<PathBuf> {
    for path in metadata_paths(repo, "relations") {
        let mut document = read_document::<RelationDocument>(&path)?;
        if matches!(&document, RelationDocument::Single(relation)
            if relation.from == from && relation.to == to
                && relation.kind == kind)
        {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
            return Ok(path);
        }
        if remove_relation_item(&mut document, from, to, kind) {
            write_document(&path, &document)?;
            return Ok(path);
        }
    }
    bail!("matching global relationship not found")
}

pub fn update_strand<F>(repo: &Repository, id: &str, mutate: F) -> Result<PathBuf>
where
    F: FnOnce(&mut Strand) -> Result<()>,
{
    let paths = metadata_paths(repo, "strands");
    let mut mutate = Some(mutate);
    for path in paths {
        let mut document = read_document::<StrandDocument>(&path)?;
        if let Some(item) = strand_items_mut(&mut document)
            .into_iter()
            .find(|item| item.id == id)
        {
            mutate
                .take()
                .context("metadata mutation was already consumed")?(item)?;
            write_document(&path, &document)?;
            return Ok(path);
        }
    }
    bail!("unknown strand {id:?}")
}

pub fn update_anchor<F>(repo: &Repository, id: &str, mutate: F) -> Result<PathBuf>
where
    F: FnOnce(&mut Anchor) -> Result<()>,
{
    let paths = metadata_paths(repo, "anchors");
    let mut mutate = Some(mutate);
    for path in paths {
        let mut document = read_document::<AnchorDocument>(&path)?;
        if let Some(item) = anchor_items_mut(&mut document)
            .into_iter()
            .find(|item| item.id == id)
        {
            mutate
                .take()
                .context("metadata mutation was already consumed")?(item)?;
            write_document(&path, &document)?;
            return Ok(path);
        }
    }
    bail!("unknown sidecar anchor {id:?}; source annotations must be edited in source")
}

pub fn remove_strand(repo: &Repository, id: &str) -> Result<PathBuf> {
    for path in metadata_paths(repo, "strands") {
        let mut document = read_document::<StrandDocument>(&path)?;
        if matches!(&document, StrandDocument::Single(strand) if strand.id == id) {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
            return Ok(path);
        }
        if remove_strand_item(&mut document, id) {
            write_document(&path, &document)?;
            return Ok(path);
        }
    }
    bail!("unknown strand {id:?}")
}

pub fn remove_anchor(repo: &Repository, id: &str) -> Result<PathBuf> {
    for path in metadata_paths(repo, "anchors") {
        let mut document = read_document::<AnchorDocument>(&path)?;
        if matches!(&document, AnchorDocument::Single(anchor) if anchor.id == id) {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove {}", path.display()))?;
            return Ok(path);
        }
        if remove_anchor_item(&mut document, id) {
            write_document(&path, &document)?;
            return Ok(path);
        }
    }
    bail!("unknown sidecar anchor {id:?}; source annotations must be edited in source")
}

pub fn remove_anchor_references(repo: &Repository, id: &str) -> Result<usize> {
    let mut changed = 0;
    for path in metadata_paths(repo, "strands") {
        let mut document = read_document::<StrandDocument>(&path)?;
        let mut document_changed = false;
        for strand in strand_items_mut(&mut document) {
            let before_members = strand.members.len();
            let before_relations = strand.relations.len();
            strand.members.retain(|member| member.anchor != id);
            strand
                .relations
                .retain(|relation| relation.from != id && relation.to != id);
            document_changed |= before_members != strand.members.len()
                || before_relations != strand.relations.len();
        }
        if document_changed {
            write_document(&path, &document)?;
            changed += 1;
        }
    }
    Ok(changed)
}

pub fn ensure_unique_strand(repo: &Repository, id: &str) -> Result<()> {
    if load(repo).strands.iter().any(|item| item.value.id == id) {
        bail!("strand {id:?} already exists");
    }
    Ok(())
}

pub fn ensure_unique_anchor(repo: &Repository, id: &str) -> Result<()> {
    if load(repo).anchors.iter().any(|item| item.value.id == id) {
        bail!("anchor {id:?} already exists");
    }
    Ok(())
}

pub fn metadata_paths(repo: &Repository, kind: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let directory = repo.metadata_dir.join(kind);
    if directory.is_dir() {
        for entry in WalkBuilder::new(&directory).hidden(false).build().flatten() {
            if entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
                && supported(entry.path())
            {
                paths.push(entry.into_path());
            }
        }
    }
    for extension in ["yaml", "yml", "json", "toml"] {
        let path = repo.metadata_dir.join(format!("{kind}.{extension}"));
        if path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

fn supported(path: &Path) -> bool {
    matches!(
        path.extension().and_then(std::ffi::OsStr::to_str),
        Some("yaml" | "yml" | "json" | "toml")
    )
}

fn read_document<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_document_text(path, &text)
}

fn parse_document_text<T: DeserializeOwned>(path: &Path, text: &str) -> Result<T> {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("yaml" | "yml") => serde_yaml_ng::from_str(text)
            .with_context(|| format!("invalid YAML in {}", path.display())),
        Some("json") => serde_json::from_str(text)
            .with_context(|| format!("invalid JSON in {}", path.display())),
        Some("toml") => {
            toml::from_str(text).with_context(|| format!("invalid TOML in {}", path.display()))
        }
        _ => bail!("unsupported metadata format: {}", path.display()),
    }
}

fn write_document<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().context("metadata file has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let bytes = match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("yaml" | "yml") => serde_yaml_ng::to_string(value)
            .context("failed to encode YAML")?
            .into_bytes(),
        Some("json") => {
            let mut bytes = serde_json::to_vec_pretty(value).context("failed to encode JSON")?;
            bytes.push(b'\n');
            bytes
        }
        Some("toml") => toml::to_string_pretty(value)
            .context("failed to encode TOML")?
            .into_bytes(),
        _ => bail!("unsupported metadata format: {}", path.display()),
    };
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temporary file in {}", parent.display()))?;
    set_metadata_permissions(temporary.as_file(), path)?;
    temporary
        .write_all(&bytes)
        .context("failed to write metadata")?;
    temporary.flush().context("failed to flush metadata")?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

fn set_metadata_permissions(file: &fs::File, existing: &Path) -> Result<()> {
    if let Ok(metadata) = fs::metadata(existing) {
        file.set_permissions(metadata.permissions())?;
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
    }
    Ok(())
}

fn unique_path(directory: &Path, id: &str) -> PathBuf {
    let mut slug: String = id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if slug.is_empty() || slug != id {
        let digest = blake3::hash(id.as_bytes()).to_hex();
        slug.push('-');
        slug.push_str(&digest[..8]);
    }
    directory.join(format!("{slug}.yaml"))
}

fn strand_items(document: &StrandDocument) -> Vec<&Strand> {
    match document {
        StrandDocument::Wrapped { strands } | StrandDocument::List(strands) => {
            strands.iter().collect()
        }
        StrandDocument::Single(strand) => vec![strand.as_ref()],
    }
}

fn strand_items_mut(document: &mut StrandDocument) -> Vec<&mut Strand> {
    match document {
        StrandDocument::Wrapped { strands } | StrandDocument::List(strands) => {
            strands.iter_mut().collect()
        }
        StrandDocument::Single(strand) => vec![strand.as_mut()],
    }
}

fn anchor_items(document: &AnchorDocument) -> Vec<&Anchor> {
    match document {
        AnchorDocument::Wrapped { anchors } | AnchorDocument::List(anchors) => {
            anchors.iter().collect()
        }
        AnchorDocument::Single(anchor) => vec![anchor.as_ref()],
    }
}

fn anchor_items_mut(document: &mut AnchorDocument) -> Vec<&mut Anchor> {
    match document {
        AnchorDocument::Wrapped { anchors } | AnchorDocument::List(anchors) => {
            anchors.iter_mut().collect()
        }
        AnchorDocument::Single(anchor) => vec![anchor.as_mut()],
    }
}

fn relation_items(document: &RelationDocument) -> Vec<&Relation> {
    match document {
        RelationDocument::Wrapped { relations } | RelationDocument::List(relations) => {
            relations.iter().collect()
        }
        RelationDocument::Single(relation) => vec![relation.as_ref()],
    }
}

fn remove_strand_item(document: &mut StrandDocument, id: &str) -> bool {
    match document {
        StrandDocument::Wrapped { strands } | StrandDocument::List(strands) => {
            let before = strands.len();
            strands.retain(|strand| strand.id != id);
            before != strands.len()
        }
        StrandDocument::Single(strand) => strand.id == id,
    }
}

fn remove_anchor_item(document: &mut AnchorDocument, id: &str) -> bool {
    match document {
        AnchorDocument::Wrapped { anchors } | AnchorDocument::List(anchors) => {
            let before = anchors.len();
            anchors.retain(|anchor| anchor.id != id);
            before != anchors.len()
        }
        AnchorDocument::Single(anchor) => anchor.id == id,
    }
}

fn remove_relation_item(document: &mut RelationDocument, from: &str, to: &str, kind: &str) -> bool {
    match document {
        RelationDocument::Wrapped { relations } | RelationDocument::List(relations) => {
            let before = relations.len();
            relations.retain(|relation| {
                !(relation.from == from && relation.to == to && relation.kind == kind)
            });
            before != relations.len()
        }
        RelationDocument::Single(relation) => {
            relation.from == from && relation.to == to && relation.kind == kind
        }
    }
}

fn find_id_line(path: &Path, id: &str) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    let quoted = format!("\"{id}\"");
    text.lines()
        .position(|line| {
            let trimmed = line.trim();
            (trimmed.starts_with("id:") || trimmed.starts_with("\"id\""))
                && (trimmed.contains(id) || trimmed.contains(&quoted))
        })
        .and_then(|line| u32::try_from(line + 1).ok())
}

fn parse_diagnostic(repo: &Repository, path: &Path, error: &anyhow::Error) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: "metadata-parse".into(),
        message: format!("{error:#}"),
        path: Some(repo.relative(path)),
        line: None,
        hint: Some("Correct the document or use YAML, JSON, or TOML syntax".into()),
    }
}
