use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{model::Attributes, repo::Repository};

fn config_version() -> u32 {
    1
}

fn default_max_file_bytes() -> u64 {
    4 * 1024 * 1024
}

fn default_auto_refresh() -> bool {
    true
}

fn default_cache_path() -> String {
    "cache/index.json".into()
}

fn default_review_path() -> String {
    "reviews".into()
}

fn default_depth() -> usize {
    1
}

fn default_anchor_markers() -> Vec<String> {
    vec!["@strandmap anchor".into(), "@anchor".into()]
}

fn default_strand_markers() -> Vec<String> {
    vec!["@strandmap strand".into(), "@strand".into()]
}

fn default_relation_markers() -> Vec<String> {
    vec!["@strandmap relation".into(), "@relation".into()]
}

fn default_gap() -> usize {
    3
}

fn default_excludes() -> Vec<String> {
    vec![
        ".git/**".into(),
        ".strandmap/**".into(),
        "**/.strandmap/**".into(),
        "target/**".into(),
        "**/target/**".into(),
        "node_modules/**".into(),
        "**/node_modules/**".into(),
        "vendor/**".into(),
        "dist/**".into(),
        "build/**".into(),
        "*.min.js".into(),
        "*.map".into(),
        "*.lock".into(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Config {
    #[serde(default = "config_version")]
    pub version: u32,
    #[serde(default)]
    pub scan: ScanConfig,
    #[serde(default)]
    pub annotations: AnnotationConfig,
    #[serde(default)]
    pub index: IndexConfig,
    #[serde(default)]
    pub traversal: TraversalConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub reviews: ReviewConfig,
    #[serde(default)]
    pub git: GitConfig,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: config_version(),
            scan: ScanConfig::default(),
            annotations: AnnotationConfig::default(),
            index: IndexConfig::default(),
            traversal: TraversalConfig::default(),
            context: ContextConfig::default(),
            reviews: ReviewConfig::default(),
            git: GitConfig::default(),
            attributes: Attributes::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ContextConfig {
    #[serde(default)]
    pub include_rust_tests: bool,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ScanConfig {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default = "default_excludes")]
    pub exclude: Vec<String>,
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub follow_symlinks: bool,
    #[serde(default = "default_true")]
    pub respect_gitignore: bool,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            exclude: default_excludes(),
            max_file_bytes: default_max_file_bytes(),
            hidden: false,
            follow_symlinks: false,
            respect_gitignore: true,
            attributes: Attributes::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AnnotationConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_anchor_markers")]
    pub anchor_markers: Vec<String>,
    #[serde(default = "default_strand_markers")]
    pub strand_markers: Vec<String>,
    #[serde(default = "default_relation_markers")]
    pub relation_markers: Vec<String>,
    #[serde(default = "default_gap")]
    pub anchor_block_gap: usize,
    #[serde(default = "default_implicit")]
    pub implicit_anchors: bool,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

const fn default_enabled() -> bool {
    true
}

const fn default_implicit() -> bool {
    true
}

impl Default for AnnotationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            anchor_markers: default_anchor_markers(),
            strand_markers: default_strand_markers(),
            relation_markers: default_relation_markers(),
            anchor_block_gap: default_gap(),
            implicit_anchors: true,
            attributes: Attributes::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct IndexConfig {
    #[serde(default = "default_cache_path")]
    pub path: String,
    #[serde(default = "default_auto_refresh")]
    pub auto_refresh: bool,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            path: default_cache_path(),
            auto_refresh: true,
            attributes: Attributes::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TraversalConfig {
    #[serde(default = "default_depth")]
    pub depth: usize,
    #[serde(default)]
    pub relation_kinds: BTreeSet<String>,
    #[serde(default = "default_true")]
    pub include_optional_members: bool,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

impl Default for TraversalConfig {
    fn default() -> Self {
        Self {
            depth: default_depth(),
            relation_kinds: BTreeSet::new(),
            include_optional_members: true,
            attributes: Attributes::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewConfig {
    #[serde(default = "default_review_path")]
    pub path: String,
    #[serde(default)]
    pub allowed_dispositions: BTreeSet<String>,
    #[serde(default = "default_require_all")]
    pub require_all_members: bool,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

const fn default_require_all() -> bool {
    false
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            path: default_review_path(),
            allowed_dispositions: BTreeSet::new(),
            require_all_members: false,
            attributes: Attributes::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GitConfig {
    #[serde(default = "default_git_diff")]
    pub default_diff: String,
    #[serde(default = "default_true")]
    pub detect_renames: bool,
    #[serde(default = "default_true")]
    pub include_untracked: bool,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

fn default_git_diff() -> String {
    "HEAD".into()
}

const fn default_true() -> bool {
    true
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            default_diff: default_git_diff(),
            detect_renames: true,
            include_untracked: true,
            attributes: Attributes::new(),
        }
    }
}

impl Config {
    pub fn load(repo: &Repository) -> Result<(Self, std::path::PathBuf)> {
        let candidates = ["config.yaml", "config.yml", "config.json", "config.toml"];
        for name in candidates {
            let path = repo.metadata_dir.join(name);
            if path.is_file() {
                let bytes = fs::read(&path)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                let config = parse_format(&path, &bytes)?;
                return Ok((config, path));
            }
        }
        bail!(
            "no Strandmap configuration found in {}; run `strandmap init`",
            repo.metadata_dir.display()
        )
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!(
                "unsupported config version {}; this binary supports version 1",
                self.version
            );
        }
        if self.scan.max_file_bytes == 0 {
            bail!("scan.max_file_bytes must be greater than zero");
        }
        validate_relative_path(&self.index.path, "index.path")?;
        validate_relative_path(&self.reviews.path, "reviews.path")?;
        if self.index.path == self.reviews.path {
            bail!("index.path and reviews.path must be different");
        }
        if self.annotations.enabled
            && (self.annotations.anchor_markers.is_empty()
                || self.annotations.strand_markers.is_empty())
        {
            bail!("enabled annotations require anchor_markers and strand_markers");
        }
        if self
            .annotations
            .anchor_markers
            .iter()
            .chain(&self.annotations.strand_markers)
            .chain(&self.annotations.relation_markers)
            .any(|marker| marker.is_empty())
        {
            bail!("annotation markers cannot be empty");
        }
        if self.git.default_diff.trim().is_empty() {
            bail!("git.default_diff cannot be empty");
        }
        if self
            .reviews
            .allowed_dispositions
            .iter()
            .any(|value| value.trim().is_empty())
        {
            bail!("reviews.allowed_dispositions cannot contain an empty value");
        }
        Ok(())
    }
}

fn validate_relative_path(path: &str, field: &str) -> Result<()> {
    let value = Path::new(path);
    if path.trim().is_empty()
        || path == "."
        || value.is_absolute()
        || value
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        bail!("{field} must be a relative path contained by the metadata directory");
    }
    Ok(())
}

fn parse_format<T: serde::de::DeserializeOwned>(path: &Path, bytes: &[u8]) -> Result<T> {
    let text =
        std::str::from_utf8(bytes).with_context(|| format!("{} is not UTF-8", path.display()))?;
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
