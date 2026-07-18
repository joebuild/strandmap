use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::{
    config::Config,
    model::{ContextPacket, Index, Review, ReviewDisposition, ReviewStatus},
    repo::Repository,
};

pub fn start(
    repo: &Repository,
    config: &Config,
    index: &Index,
    packet: &ContextPacket,
    requested_id: Option<&str>,
) -> Result<(Review, PathBuf)> {
    let id = requested_id
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    validate_id(&id)?;
    let directory = directory(repo, config)?;
    let path = directory.join(format!("{id}.yaml"));
    if path.exists() {
        bail!("review {id:?} already exists");
    }
    let now = Utc::now();
    let strands: BTreeSet<String> = packet
        .strands
        .iter()
        .map(|strand| strand.id.clone())
        .collect();
    let anchors: BTreeSet<String> = packet
        .strands
        .iter()
        .flat_map(|strand| strand.anchors.iter().map(|anchor| anchor.id.clone()))
        .chain(
            packet
                .related_anchors
                .iter()
                .map(|anchor| anchor.id.clone()),
        )
        .collect();
    let mut required_anchors = BTreeSet::new();
    for strand in &packet.strands {
        let disposition_required = index
            .strands
            .get(&strand.id)
            .and_then(|item| item.value.on_change.as_ref())
            .and_then(|policy| policy.require_disposition)
            .unwrap_or(true);
        if !disposition_required {
            continue;
        }
        for anchor in &strand.anchors {
            let member_required = index.memberships.iter().any(|membership| {
                membership.strand.as_deref() == Some(strand.id.as_str())
                    && membership.member.anchor == anchor.id
                    && membership.member.required
            });
            if config.reviews.require_all_members || member_required {
                required_anchors.insert(anchor.id.clone());
            }
        }
    }
    if config.reviews.require_all_members {
        required_anchors.extend(
            packet
                .related_anchors
                .iter()
                .map(|anchor| anchor.id.clone()),
        );
    }
    let file_fingerprints = packet
        .changes
        .files
        .iter()
        .map(|changed| {
            (
                changed.path.clone(),
                index
                    .files
                    .get(&changed.path)
                    .and_then(|file| file.content_hash.clone()),
            )
        })
        .collect();
    let review = Review {
        schema: 1,
        id,
        status: ReviewStatus::Open,
        created_at: now,
        updated_at: now,
        completed_at: None,
        change_fingerprint: packet.changes.fingerprint.clone(),
        change_description: packet.changes.description.clone(),
        required_anchors,
        anchors,
        strands,
        file_fingerprints,
        dispositions: BTreeMap::new(),
        attributes: BTreeMap::new(),
    };
    save(&path, &review)?;
    Ok((review, path))
}

pub fn record(
    repo: &Repository,
    config: &Config,
    id: &str,
    anchor: &str,
    disposition: &str,
    note: Option<String>,
) -> Result<Review> {
    if disposition.trim().is_empty() {
        bail!("disposition cannot be empty");
    }
    if !config.reviews.allowed_dispositions.is_empty()
        && !config.reviews.allowed_dispositions.contains(disposition)
    {
        bail!(
            "disposition {disposition:?} is not allowed; configured values: {}",
            config
                .reviews
                .allowed_dispositions
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let path = review_path(repo, config, id)?;
    let mut review = read(&path)?;
    if review.status == ReviewStatus::Complete {
        bail!("review {id:?} is complete; reopen it before recording changes");
    }
    if !review.anchors.contains(anchor) {
        bail!("anchor {anchor:?} is not part of review {id:?}");
    }
    let now = Utc::now();
    review.dispositions.insert(
        anchor.into(),
        ReviewDisposition {
            disposition: disposition.into(),
            note,
            recorded_at: now,
        },
    );
    review.updated_at = now;
    save(&path, &review)?;
    Ok(review)
}

pub fn complete(
    repo: &Repository,
    config: &Config,
    index: &Index,
    id: &str,
    allow_incomplete: bool,
    allow_drift: bool,
) -> Result<Review> {
    let path = review_path(repo, config, id)?;
    let mut review = read(&path)?;
    if review.status == ReviewStatus::Complete {
        return Ok(review);
    }
    let missing: Vec<_> = review
        .required_anchors
        .iter()
        .filter(|anchor| !review.dispositions.contains_key(*anchor))
        .cloned()
        .collect();
    if !allow_incomplete && !missing.is_empty() {
        bail!(
            "review has {} missing disposition(s): {}",
            missing.len(),
            missing.join(", ")
        );
    }
    if !allow_drift {
        let drifted: Vec<_> = review
            .file_fingerprints
            .iter()
            .filter_map(|(path, expected)| {
                let actual = index
                    .files
                    .get(path)
                    .and_then(|file| file.content_hash.as_ref());
                if actual == expected.as_ref() {
                    None
                } else {
                    Some(path.clone())
                }
            })
            .collect();
        if !drifted.is_empty() {
            bail!(
                "reviewed files changed after the review began: {}; inspect them and update dispositions, or use --allow-drift",
                drifted.join(", ")
            );
        }
    }
    let now = Utc::now();
    review.status = ReviewStatus::Complete;
    review.updated_at = now;
    review.completed_at = Some(now);
    save(&path, &review)?;
    Ok(review)
}

pub fn reopen(repo: &Repository, config: &Config, id: &str) -> Result<Review> {
    let path = review_path(repo, config, id)?;
    let mut review = read(&path)?;
    let now = Utc::now();
    review.status = ReviewStatus::Open;
    review.updated_at = now;
    review.completed_at = None;
    save(&path, &review)?;
    Ok(review)
}

pub fn get(repo: &Repository, config: &Config, id: Option<&str>) -> Result<Review> {
    if let Some(id) = id {
        return read(&review_path(repo, config, id)?);
    }
    let reviews = list(repo, config)?;
    reviews
        .iter()
        .rev()
        .find(|review| review.status == ReviewStatus::Open)
        .or_else(|| reviews.last())
        .cloned()
        .context("no reviews found")
}

pub fn list(repo: &Repository, config: &Config) -> Result<Vec<Review>> {
    let directory = directory(repo, config)?;
    let mut reviews = Vec::new();
    for entry in fs::read_dir(&directory)
        .with_context(|| format!("failed to read {}", directory.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) == Some("yaml") {
            reviews.push(read(&path)?);
        }
    }
    reviews.sort_by_key(|review| review.created_at);
    Ok(reviews)
}

fn directory(repo: &Repository, config: &Config) -> Result<PathBuf> {
    let path = repo.metadata_dir.join(&config.reviews.path);
    fs::create_dir_all(&path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok(path)
}

fn review_path(repo: &Repository, config: &Config, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    let path = directory(repo, config)?.join(format!("{id}.yaml"));
    if !path.is_file() {
        bail!("unknown review {id:?}");
    }
    Ok(path)
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        bail!("review ids may contain only letters, digits, '.', '-', and '_'");
    }
    Ok(())
}

fn read(path: &Path) -> Result<Review> {
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let review: Review = serde_yaml_ng::from_str(&text)
        .with_context(|| format!("invalid review at {}", path.display()))?;
    if review.schema != 1 {
        bail!("unsupported review schema {}", review.schema);
    }
    Ok(review)
}

fn save(path: &Path, review: &Review) -> Result<()> {
    let parent = path.parent().context("review path has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_yaml_ng::to_writer(&mut temporary, review).context("failed to encode review")?;
    temporary.flush()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}
