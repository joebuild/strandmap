use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Attributes = BTreeMap<String, Value>;

fn schema_version() -> u32 {
    1
}

fn required() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<WatchMode>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WatchMode {
    #[default]
    File,
    Line,
    Range,
    Node,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Anchor {
    #[serde(default = "schema_version")]
    pub schema: u32,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<String>,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Member {
    pub anchor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default = "required")]
    pub required: bool,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Relation {
    pub from: String,
    pub to: String,
    pub kind: String,
    #[serde(default)]
    pub bidirectional: bool,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChangePolicy {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub include_roles: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub exclude_roles: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub follow_relations: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_disposition: Option<bool>,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Strand {
    #[serde(default = "schema_version")]
    pub schema: u32,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub intent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_change: Option<ChangePolicy>,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Provenance {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Indexed<T> {
    pub value: T,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct IndexedMember {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strand: Option<String>,
    #[serde(flatten)]
    pub member: Member,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct IndexedRelation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strand: Option<String>,
    #[serde(flatten)]
    pub relation: Relation,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct FileRecord {
    pub path: String,
    pub size: u64,
    pub modified_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_bloom: Vec<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rust_test_ranges: Vec<LineRange>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SourceDocument {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<Indexed<Anchor>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strands: Vec<Indexed<Strand>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memberships: Vec<IndexedMember>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<IndexedRelation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Index {
    pub schema: u32,
    pub root: String,
    pub generated_at: DateTime<Utc>,
    pub workspace_fingerprint: String,
    pub metadata_fingerprint: String,
    pub config_fingerprint: String,
    pub files: BTreeMap<String, FileRecord>,
    pub source_documents: BTreeMap<String, SourceDocument>,
    pub strands: BTreeMap<String, Indexed<Strand>>,
    pub anchors: BTreeMap<String, Indexed<Anchor>>,
    pub memberships: Vec<IndexedMember>,
    pub relations: Vec<IndexedRelation>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

impl LineRange {
    #[must_use]
    pub fn overlaps(&self, start: u32, end: u32) -> bool {
        self.start <= end && start <= self.end
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    #[serde(default)]
    pub old_path: Option<String>,
    #[serde(default)]
    pub status: ChangeStatus,
    #[serde(default)]
    pub ranges: Vec<LineRange>,
    #[serde(default)]
    pub whole_file: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeStatus {
    Added,
    Deleted,
    Renamed,
    Copied,
    #[default]
    Modified,
    Untracked,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct RemovedLine {
    pub path: String,
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ChangeSet {
    pub description: String,
    pub fingerprint: String,
    pub files: Vec<ChangedFile>,
    #[serde(default)]
    pub removed_lines: Vec<RemovedLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AffectedAnchor {
    pub id: String,
    pub reason: String,
    pub direct: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AffectedStrand {
    pub id: String,
    pub intent: String,
    pub direct: bool,
    pub anchors: Vec<AffectedAnchor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ContextPacket {
    pub changes: ChangeSet,
    pub strands: Vec<AffectedStrand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_anchors: Vec<AffectedAnchor>,
    pub unmatched_files: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewStatus {
    Open,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ReviewDisposition {
    pub disposition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Review {
    #[serde(default = "schema_version")]
    pub schema: u32,
    pub id: String,
    pub status: ReviewStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub change_fingerprint: String,
    pub change_description: String,
    pub required_anchors: BTreeSet<String>,
    pub anchors: BTreeSet<String>,
    pub strands: BTreeSet<String>,
    #[serde(default)]
    pub file_fingerprints: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub dispositions: BTreeMap<String, ReviewDisposition>,
    #[serde(default, flatten)]
    pub attributes: Attributes,
}
