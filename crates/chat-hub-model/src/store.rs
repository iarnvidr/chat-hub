use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct TagSnapshot {
    pub name: String,
    pub target_count: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct TaggedTargetSnapshot {
    pub target_kind: String,
    pub target_id: String,
    pub tags: Vec<String>,
    pub tag_revision: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct TaggedTargetsPage {
    pub tag_revision: i64,
    pub items: Vec<TaggedTargetSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    Imported { conversation_id: String },
    Updated { conversation_id: String },
    Unchanged { conversation_id: String },
}

#[derive(Clone, Debug)]
pub struct LibrarySnapshot {
    pub library_revision: i64,
    pub conversations: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct ListKey {
    pub pinned: bool,
    pub updated_at_ms: Option<i64>,
    pub id: String,
}

#[derive(Clone, Debug)]
pub struct SearchKey {
    pub score: f64,
    pub conversation_id: String,
    pub node_id: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct PageScope<'a> {
    pub source: &'a str,
    pub project_key: Option<&'a str>,
    pub tag: Option<&'a str>,
}

#[derive(Clone, Debug)]
pub struct ListPageSnapshot {
    pub library_revision: i64,
    pub tag_revision: Option<i64>,
    pub items: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct SearchPageSnapshot {
    pub library_revision: i64,
    pub tag_revision: Option<i64>,
    pub hits: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct ProjectSnapshot {
    pub library_revision: i64,
    pub projects: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct ProjectRenameSnapshot {
    pub library_revision: i64,
    pub project: Value,
}

#[derive(Clone, Debug)]
pub struct ProjectMembership {
    pub source_id: String,
    pub project_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProjectTitle {
    pub source_id: String,
    pub title: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DetailSnapshot {
    pub library_revision: i64,
    pub conversation: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct SearchSnapshot {
    pub library_revision: i64,
    pub hits: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub provider: String,
    pub account_scope: String,
    pub source_id: String,
    pub title: String,
    pub url: Option<String>,
    pub current_node: Option<String>,
    pub graph: Vec<GraphNode>,
    pub raw_payload: Value,
    pub source_time: Option<String>,
    pub observed_ms: i64,
    pub parser: String,
    pub completeness: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct GraphNode {
    pub source_node_id: String,
    pub source_message_id: Option<String>,
    pub parent_id: Option<String>,
    pub role: Option<String>,
    pub author: Option<Value>,
    pub create_time: Option<Value>,
    pub parts: Vec<Value>,
}
