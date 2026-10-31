//! Public DTOs for the persistent, rebuildable chat overview catalog.
use serde::Serialize;

pub const CATALOG_INDEX_VERSION: i64 = 3;

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogIndexStatus {
    pub index_version: i64,
    pub library_revision: i64,
    pub indexed_revision: Option<i64>,
    pub rebuilt_at_ms: Option<i64>,
    pub stale: bool,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogSourceStats {
    pub provider: String,
    pub chats: i64,
    pub messages: i64,
    pub graph_nodes: i64,
    pub branch_points: i64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogProvenanceCoverage {
    pub chats_with_source_time: i64,
    pub chats_with_parser: i64,
    pub chats_with_completeness: i64,
    pub cached_archives: i64,
    pub full_graphs: i64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogStats {
    pub chats: i64,
    pub projects: i64,
    pub shared_keywords: i64,
    pub messages: i64,
    pub graph_nodes: i64,
    pub branch_points: i64,
    pub entities: i64,
    pub repositories: i64,
    pub pull_requests: i64,
    pub issues: i64,
    pub domains: i64,
    pub links: i64,
    pub file_artifacts: i64,
    pub latest_activity_ms: Option<i64>,
    pub source_coverage: Vec<CatalogSourceStats>,
    pub provenance: CatalogProvenanceCoverage,
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CatalogEntityKind {
    Repository,
    PullRequest,
    Issue,
    Domain,
    #[default]
    Link,
    FileArtifact,
}

impl CatalogEntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::PullRequest => "pull_request",
            Self::Issue => "issue",
            Self::Domain => "domain",
            Self::Link => "link",
            Self::FileArtifact => "file_artifact",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "repository" => Some(Self::Repository),
            "pull_request" => Some(Self::PullRequest),
            "issue" => Some(Self::Issue),
            "domain" => Some(Self::Domain),
            "link" => Some(Self::Link),
            "file_artifact" => Some(Self::FileArtifact),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogEntity {
    pub id: String,
    pub kind: CatalogEntityKind,
    pub label: String,
    pub canonical_url: Option<String>,
    pub domain: Option<String>,
    pub repository_id: Option<String>,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub provider_file_id: Option<String>,
    pub canonical_locator: Option<String>,
    /// `link_only` or `metadata_present` for files; bytes are never implied.
    pub artifact_state: Option<String>,
    pub content_saved: bool,
    pub reference_count: i64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub produced_reference_count: i64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub attachment_reference_count: i64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub mentioned_reference_count: i64,
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogEntityReference {
    pub conversation_id: String,
    pub source_id: String,
    pub title: String,
    pub provider: String,
    pub message_id: Option<String>,
    pub source_message_id: Option<String>,
    pub source_node_id: Option<String>,
    pub role: Option<String>,
    pub source_field: String,
    pub provenance: String,
    pub relation: String,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogEntityDetail {
    pub entity: CatalogEntity,
    pub references: Vec<CatalogEntityReference>,
    pub references_truncated: bool,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogEntityPage {
    pub index: CatalogIndexStatus,
    pub source: String,
    pub project_key: Option<String>,
    pub conversation_id: Option<String>,
    pub kind: Option<String>,
    pub artifact_state: Option<String>,
    pub tag: Option<String>,
    pub limit: usize,
    pub next_cursor: Option<String>,
    pub total: i64,
    /// True when one or more matching chats exceeded the bounded entity scan.
    pub coverage_truncated: bool,
    pub items: Vec<CatalogEntity>,
}

/// Store-level filters for a whole-index entity page. Query matching is applied
/// before total counts and keyset pagination.
#[derive(Clone, Copy, Debug, Default)]
pub struct CatalogEntityQuery<'a> {
    pub source: &'a str,
    pub project_key: Option<&'a str>,
    pub conversation_id: Option<&'a str>,
    pub kind: Option<CatalogEntityKind>,
    pub artifact_state: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub query: Option<&'a str>,
    pub cursor: Option<&'a str>,
    pub limit: usize,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogOverview {
    pub index: CatalogIndexStatus,
    pub stats: CatalogStats,
}

/// A chat node points back to the exact local conversation and provider source ID.
/// Project and keyword nodes omit those fields. `basis` is populated only on edges.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogNode {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub label_source: Option<String>,
    pub provider: Option<String>,
    pub account_scope: Option<String>,
    pub conversation_id: Option<String>,
    pub source_id: Option<String>,
    pub project_key: Option<String>,
    pub keyword: Option<String>,
    pub updated_at_ms: Option<i64>,
    pub graph_node_count: Option<i64>,
    pub message_count: Option<i64>,
    pub branch_count: Option<i64>,
    pub source_time: Option<String>,
    pub observed_ms: Option<i64>,
    pub parser: Option<String>,
    pub completeness: Option<String>,
    pub cached_archive: Option<bool>,
    pub sample_bytes: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogEdge {
    pub source: String,
    pub target: String,
    /// Relationship basis: `project` or `shared_keyword`.
    pub basis: String,
    /// Plain-language basis and bounded evidence for the connection.
    pub reason: Option<String>,
    pub evidence: Vec<String>,
    /// Relative integer strength; project membership is the strongest basis.
    pub score: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogGraph {
    pub index: CatalogIndexStatus,
    pub stats: CatalogStats,
    pub truncated: bool,
    /// Chats omitted from this bounded connected map, out of the filtered index.
    pub unmapped_count: i64,
    pub focus_id: Option<String>,
    pub nodes: Vec<CatalogNode>,
    pub edges: Vec<CatalogEdge>,
}
