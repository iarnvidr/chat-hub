//! Public DTOs for the persistent, rebuildable chat overview catalog.
use serde::Serialize;

pub const CATALOG_INDEX_VERSION: i64 = 2;

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
    pub latest_activity_ms: Option<i64>,
    pub source_coverage: Vec<CatalogSourceStats>,
    pub provenance: CatalogProvenanceCoverage,
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
