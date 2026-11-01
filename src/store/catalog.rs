use crate::catalog::{
    CatalogEdge, CatalogEntity, CatalogEntityDetail, CatalogEntityKind, CatalogEntityPage,
    CatalogEntityQuery, CatalogEntityReference, CatalogGraph, CatalogIndexStatus, CatalogNode,
    CatalogOverview, CatalogProvenanceCoverage, CatalogSourceStats, CatalogStats,
    CATALOG_INDEX_VERSION,
};
use crate::error::{Error, Result};
use crate::store::Store;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_SAMPLE_BYTES: usize = 64 * 1024;
const SAMPLE_CHUNK_BYTES: usize = 4 * 1024;
const MAX_SAMPLE_ROWS: usize = MAX_SAMPLE_BYTES / SAMPLE_CHUNK_BYTES;
const REBUILD_BATCH_CHATS: usize = 64;
const MAX_TERMS_PER_CHAT: usize = 10;
const MAX_CHATS_IN_GRAPH: usize = 100;
const MAX_GLOBAL_GRAPH_CANDIDATES: usize = 20_000;
const MAX_GLOBAL_GRAPH_EDGES: usize = 30_000;
const MAX_KEYWORD_POSTING: usize = 256;
const PROJECT_CANDIDATE_NEIGHBORS: usize = 3;
const MAX_NEIGHBOR_DEGREE: usize = 4;
const KEYWORD_CANDIDATE_SCAN: usize = 100;
const MAX_EDGES_IN_GRAPH: usize = MAX_CHATS_IN_GRAPH * MAX_NEIGHBOR_DEGREE / 2;
const MAX_ENTITY_SCAN_PER_CHAT: usize = 32 * 1024 * 1024;
const MAX_ENTITY_SCAN_PER_FIELD: usize = 8 * 1024 * 1024;
const MAX_ENTITY_OCCURRENCES_PER_CHAT: usize = 2_000;
const MAX_ENTITY_REFS_PER_ENTITY: usize = 20_000;

const CREATE_CATALOG_TABLES: &str = "
CREATE TABLE IF NOT EXISTS catalog_meta(
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  index_version INTEGER NOT NULL,
  library_revision INTEGER NOT NULL,
  rebuilt_at_ms INTEGER NOT NULL,
  chat_count INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS catalog_chats(
  catalog_id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  account_scope TEXT NOT NULL,
  source_id TEXT NOT NULL,
  title TEXT NOT NULL,
  updated_at_ms INTEGER,
  project_key TEXT,
  project_label TEXT,
  project_label_source TEXT,
  graph_node_count INTEGER NOT NULL,
  message_count INTEGER NOT NULL,
  branch_count INTEGER NOT NULL,
  source_time TEXT,
  observed_ms INTEGER,
  parser TEXT,
  completeness TEXT,
  sample_bytes INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS catalog_chats_source_activity ON catalog_chats(provider,updated_at_ms DESC,catalog_id);
CREATE INDEX IF NOT EXISTS catalog_chats_project_activity ON catalog_chats(project_key,updated_at_ms DESC,catalog_id);
CREATE TABLE IF NOT EXISTS catalog_terms(
  term_id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  account_scope TEXT NOT NULL,
  term TEXT NOT NULL,
  chat_count INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS catalog_terms_rank ON catalog_terms(provider,account_scope,chat_count DESC,term COLLATE NOCASE);
CREATE TABLE IF NOT EXISTS catalog_edges(
  source_id TEXT NOT NULL,
  target_id TEXT NOT NULL,
  basis TEXT NOT NULL CHECK(basis IN ('project','shared_keyword')),
  PRIMARY KEY(source_id,target_id,basis)
);
CREATE INDEX IF NOT EXISTS catalog_edges_target ON catalog_edges(target_id,basis,source_id);
CREATE TABLE IF NOT EXISTS catalog_entities(
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  account_scope TEXT NOT NULL,
  kind TEXT NOT NULL,
  label TEXT NOT NULL,
  canonical_url TEXT,
  domain TEXT,
  repository_id TEXT,
  filename TEXT,
  mime_type TEXT,
  provider_file_id TEXT,
  canonical_locator TEXT,
  artifact_state TEXT,
  content_saved INTEGER NOT NULL DEFAULT 0,
  reference_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS catalog_entities_kind ON catalog_entities(kind,id);
CREATE TABLE IF NOT EXISTS catalog_entity_refs(
  entity_id TEXT NOT NULL REFERENCES catalog_entities(id) ON DELETE CASCADE,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  source_id TEXT NOT NULL,
  title TEXT NOT NULL,
  provider TEXT NOT NULL,
  message_id TEXT,
  source_message_id TEXT,
  source_node_id TEXT NOT NULL,
  role TEXT,
  source_field TEXT NOT NULL,
  provenance TEXT NOT NULL,
  relation TEXT NOT NULL,
  PRIMARY KEY(entity_id,conversation_id,source_field,relation,source_node_id)
);
CREATE INDEX IF NOT EXISTS catalog_entity_refs_chat ON catalog_entity_refs(conversation_id,entity_id);
CREATE TABLE IF NOT EXISTS catalog_scan_coverage(
  conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
  truncated INTEGER NOT NULL
);
";

#[derive(Debug)]
struct SourceChat {
    conversation_id: String,
    provider: String,
    account_scope: String,
    source_id: String,
    title_bytes: Vec<u8>,
    updated_at_ms: Option<i64>,
    project_key: Option<String>,
    project_local_name: Option<String>,
    project_title: Option<String>,
    graph_node_count: i64,
    message_count: i64,
    branch_count: i64,
    source_time: Option<String>,
    observed_ms: Option<i64>,
    parser: Option<String>,
    completeness: Option<String>,
}

#[derive(Debug)]
struct IndexedChat {
    catalog_id: String,
    conversation_id: String,
    provider: String,
    account_scope: String,
    source_id: String,
    title: String,
    updated_at_ms: Option<i64>,
    project_key: Option<String>,
    project_label: Option<String>,
    project_label_source: Option<String>,
    graph_node_count: i64,
    message_count: i64,
    branch_count: i64,
    source_time: Option<String>,
    observed_ms: Option<i64>,
    parser: Option<String>,
    completeness: Option<String>,
    sample_bytes: i64,
}

impl Store {
    /// Rebuild only derived catalog rows. It never changes source conversations,
    /// projects, message graphs, or the library revision.
    pub fn rebuild_catalog_index(&mut self) -> Result<CatalogIndexStatus> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(CREATE_CATALOG_TABLES)?;
        let library_revision = current_revision(&tx)?;
        tx.execute("DELETE FROM catalog_edges", [])?;
        tx.execute("DELETE FROM catalog_entity_refs", [])?;
        tx.execute("DELETE FROM catalog_entities", [])?;
        tx.execute("DELETE FROM catalog_scan_coverage", [])?;
        tx.execute("DELETE FROM catalog_terms", [])?;
        tx.execute("DELETE FROM catalog_chats", [])?;

        let mut indexed_count = 0_i64;
        let mut after_id: Option<String> = None;
        loop {
            let batch = read_chat_batch(&tx, after_id.as_deref())?;
            if batch.is_empty() {
                break;
            }
            after_id = batch.last().map(|chat| chat.conversation_id.clone());
            // At most 64 conversations' bounded metadata is retained at once.
            // Bodies are never returned whole from SQLite.
            for chat in batch {
                let title = String::from_utf8_lossy(&chat.title_bytes).trim().to_owned();
                let catalog_id =
                    chat_catalog_id(&chat.provider, &chat.account_scope, &chat.source_id);
                let (project_label, project_label_source) = project_label(
                    chat.project_local_name.as_deref(),
                    chat.project_title.as_deref(),
                );
                let (sample, sample_bytes) = read_bounded_sample(&tx, &chat.conversation_id)?;
                let terms = choose_keywords(&title, &sample);
                index_chat_entities(&tx, &chat, &title)?;

                tx.execute(
                    "INSERT INTO catalog_chats(catalog_id,conversation_id,provider,account_scope,source_id,title,updated_at_ms,project_key,project_label,project_label_source,graph_node_count,message_count,branch_count,source_time,observed_ms,parser,completeness,sample_bytes)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                    params![
                        catalog_id,
                        chat.conversation_id,
                        chat.provider,
                        chat.account_scope,
                        chat.source_id,
                        title,
                        chat.updated_at_ms,
                        chat.project_key,
                        project_label,
                        project_label_source,
                        chat.graph_node_count,
                        chat.message_count,
                        chat.branch_count,
                        chat.source_time,
                        chat.observed_ms,
                        chat.parser,
                        chat.completeness,
                        sample_bytes as i64,
                    ],
                )?;
                if let Some(project_key) = chat.project_key.as_deref() {
                    tx.execute(
                        "INSERT INTO catalog_edges(source_id,target_id,basis) VALUES(?1,?2,'project')",
                        params![catalog_id, project_catalog_id(project_key)],
                    )?;
                }
                for term in terms {
                    let term_id = keyword_catalog_id(&chat.provider, &chat.account_scope, &term);
                    tx.execute(
                        "INSERT INTO catalog_terms(term_id,provider,account_scope,term,chat_count) VALUES(?1,?2,?3,?4,1)
                         ON CONFLICT(term_id) DO UPDATE SET chat_count=chat_count+1",
                        params![term_id, chat.provider, chat.account_scope, term],
                    )?;
                    tx.execute(
                        "INSERT INTO catalog_edges(source_id,target_id,basis) VALUES(?1,?2,'shared_keyword')",
                        params![catalog_id, term_id],
                    )?;
                }
                indexed_count += 1;
            }
        }

        // A keyword relationship requires the same normalized term to occur in
        // at least two chats owned by the same provider/account scope.
        tx.execute(
            "DELETE FROM catalog_edges WHERE basis='shared_keyword' AND target_id IN (SELECT term_id FROM catalog_terms WHERE chat_count<2)",
            [],
        )?;
        tx.execute("DELETE FROM catalog_terms WHERE chat_count<2", [])?;
        let rebuilt_at_ms = now_ms();
        tx.execute(
            "INSERT INTO catalog_meta(singleton,index_version,library_revision,rebuilt_at_ms,chat_count)
             VALUES(1,?1,?2,?3,?4)
             ON CONFLICT(singleton) DO UPDATE SET index_version=excluded.index_version,library_revision=excluded.library_revision,rebuilt_at_ms=excluded.rebuilt_at_ms,chat_count=excluded.chat_count",
            params![CATALOG_INDEX_VERSION, library_revision, rebuilt_at_ms, indexed_count],
        )?;
        let status = index_status(&tx)?;
        tx.commit()?;
        Ok(status)
    }

    pub fn catalog_overview(
        &self,
        source: &str,
        project_key: Option<&str>,
    ) -> Result<CatalogOverview> {
        let source_filter = source_filter(source)?;
        validate_project_filter(project_key)?;
        let tx = self.connection.unchecked_transaction()?;
        let index = index_status(&tx)?;
        let stats = if catalog_tables_exist(&tx)? {
            read_stats(&tx, source_filter, project_key)?
        } else {
            CatalogStats::default()
        };
        tx.commit()?;
        Ok(CatalogOverview { index, stats })
    }

    /// List explicitly extracted entities with keyset pagination. Entity IDs are opaque;
    /// file artifacts are scoped to their source chat/message and are never merged by name.
    #[allow(clippy::too_many_arguments)]
    pub fn catalog_entities(
        &self,
        source: &str,
        project_key: Option<&str>,
        conversation_id: Option<&str>,
        kind: Option<CatalogEntityKind>,
        artifact_state: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<CatalogEntityPage> {
        self.catalog_entities_search(CatalogEntityQuery {
            source,
            project_key,
            conversation_id,
            kind,
            artifact_state,
            cursor,
            limit,
            ..CatalogEntityQuery::default()
        })
    }

    /// Like `catalog_entities`, with an exact normalized local tag filter applied
    /// before count and cursor pagination.
    #[allow(clippy::too_many_arguments)]
    pub fn catalog_entities_tagged(
        &self,
        source: &str,
        project_key: Option<&str>,
        conversation_id: Option<&str>,
        kind: Option<CatalogEntityKind>,
        artifact_state: Option<&str>,
        tag: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<CatalogEntityPage> {
        self.catalog_entities_search(CatalogEntityQuery {
            source,
            project_key,
            conversation_id,
            kind,
            artifact_state,
            tag,
            cursor,
            limit,
            ..CatalogEntityQuery::default()
        })
    }

    /// Search the full entity index, applying text, source, project, tag, and
    /// category filters before page counts and keyset pagination.
    pub fn catalog_entities_search(
        &self,
        options: CatalogEntityQuery<'_>,
    ) -> Result<CatalogEntityPage> {
        let CatalogEntityQuery {
            source,
            project_key,
            conversation_id,
            kind,
            artifact_state,
            tag,
            query,
            cursor,
            limit,
        } = options;
        let source_filter = source_filter(source)?;
        validate_project_filter(project_key)?;
        validate_entity_filters(conversation_id, artifact_state, cursor)?;
        let query = query.map(str::trim).filter(|value| !value.is_empty());
        if query.is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control)) {
            return Err(Error::Configuration("invalid catalog entity query".into()));
        }
        let tag_key = tag.map(crate::store::canonical_tag_key).transpose()?;
        if limit == 0 {
            return Err(Error::Configuration(
                "entity page limit must be positive".into(),
            ));
        }
        let limit = limit.min(100);
        let tx = self.connection.unchecked_transaction()?;
        let index = index_status(&tx)?;
        if !entity_tables_exist(&tx)? {
            tx.commit()?;
            return Ok(CatalogEntityPage {
                index,
                source: source.into(),
                project_key: project_key.map(str::to_owned),
                conversation_id: conversation_id.map(str::to_owned),
                kind: kind.map(|v| v.as_str().into()),
                artifact_state: artifact_state.map(str::to_owned),
                tag: tag.map(str::to_owned),
                limit,
                ..CatalogEntityPage::default()
            });
        }
        let where_sql = entity_filter_sql(source_filter)
            .replace("e.artifact_state=?4", "(e.artifact_state=?4 OR (?4='saved_locally' AND EXISTS(SELECT 1 FROM artifact_contents ac WHERE ac.entity_id=e.id)))");
        let count_sql = format!(
            "SELECT count(DISTINCT e.id) FROM catalog_entities e JOIN catalog_entity_refs r ON r.entity_id=e.id JOIN catalog_chats c ON c.conversation_id=r.conversation_id WHERE {where_sql}"
        );
        let total: i64 = tx.query_row(
            &count_sql,
            params![
                project_key,
                conversation_id,
                kind.map(|v| v.as_str()),
                artifact_state,
                tag_key.as_deref(),
                query,
                Option::<&str>::None,
            ],
            |row| row.get(0),
        )?;
        let sql = format!(
            "SELECT DISTINCT e.id,e.kind,e.label,e.canonical_url,e.domain,e.repository_id,e.filename,e.mime_type,e.provider_file_id,e.canonical_locator,e.artifact_state,e.content_saved,e.reference_count,(SELECT count(*) FROM catalog_entity_refs rr WHERE rr.entity_id=e.id AND rr.relation='produces'),(SELECT count(*) FROM catalog_entity_refs rr WHERE rr.entity_id=e.id AND rr.relation='attaches'),(SELECT count(*) FROM catalog_entity_refs rr WHERE rr.entity_id=e.id AND rr.relation='mentions') FROM catalog_entities e JOIN catalog_entity_refs r ON r.entity_id=e.id JOIN catalog_chats c ON c.conversation_id=r.conversation_id WHERE {where_sql} ORDER BY e.id LIMIT ?8"
        );
        let mut statement = tx.prepare(&sql)?;
        let mut items = statement
            .query_map(
                params![
                    project_key,
                    conversation_id,
                    kind.map(|v| v.as_str()),
                    artifact_state,
                    tag_key.as_deref(),
                    query,
                    cursor,
                    (limit + 1) as i64
                ],
                entity_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        for entity in &mut items {
            apply_saved_artifact_state(&tx, entity)?;
        }
        let has_more = items.len() > limit;
        if has_more {
            items.truncate(limit);
        }
        let next_cursor = if has_more {
            items.last().map(|entity| entity.id.clone())
        } else {
            None
        };
        let coverage_project_filter = project_filter_sql("c", "?1");
        let coverage_sql = format!(
            "SELECT EXISTS(SELECT 1 FROM catalog_scan_coverage x JOIN catalog_chats c ON c.conversation_id=x.conversation_id WHERE x.truncated=1 AND ({source_filter}) AND {coverage_project_filter} AND (?2 IS NULL OR c.conversation_id=?2))"
        );
        let coverage_truncated: bool = tx
            .query_row(
                &coverage_sql,
                params![project_key, conversation_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v != 0)?;
        tx.commit()?;
        Ok(CatalogEntityPage {
            index,
            source: source.into(),
            project_key: project_key.map(str::to_owned),
            conversation_id: conversation_id.map(str::to_owned),
            kind: kind.map(|v| v.as_str().into()),
            artifact_state: artifact_state.map(str::to_owned),
            tag: tag.map(str::to_owned),
            limit,
            next_cursor,
            total,
            coverage_truncated,
            items,
        })
    }

    /// Return an entity and a bounded list of exact source-chat/message references.
    pub fn catalog_entity(&self, id: &str) -> Result<Option<CatalogEntityDetail>> {
        if id.trim().is_empty() || id.len() > 256 {
            return Err(Error::Configuration("invalid catalog entity id".into()));
        }
        let tx = self.connection.unchecked_transaction()?;
        if !entity_tables_exist(&tx)? {
            tx.commit()?;
            return Ok(None);
        }
        let entity = tx.query_row(
            "SELECT id,kind,label,canonical_url,domain,repository_id,filename,mime_type,provider_file_id,canonical_locator,artifact_state,content_saved,reference_count,(SELECT count(*) FROM catalog_entity_refs rr WHERE rr.entity_id=catalog_entities.id AND rr.relation='produces'),(SELECT count(*) FROM catalog_entity_refs rr WHERE rr.entity_id=catalog_entities.id AND rr.relation='attaches'),(SELECT count(*) FROM catalog_entity_refs rr WHERE rr.entity_id=catalog_entities.id AND rr.relation='mentions') FROM catalog_entities WHERE id=?1",
            [id], entity_from_row,
        ).optional()?;
        let Some(mut entity) = entity else {
            tx.commit()?;
            return Ok(None);
        };
        apply_saved_artifact_state(&tx, &mut entity)?;
        let mut statement = tx.prepare("SELECT conversation_id,source_id,title,provider,message_id,source_message_id,source_node_id,role,source_field,provenance,relation FROM catalog_entity_refs WHERE entity_id=?1 ORDER BY conversation_id,source_node_id LIMIT 101")?;
        let mut references = statement
            .query_map([id], |row| {
                Ok(CatalogEntityReference {
                    conversation_id: row.get(0)?,
                    source_id: row.get(1)?,
                    title: row.get(2)?,
                    provider: row.get(3)?,
                    message_id: row.get(4)?,
                    source_message_id: row.get(5)?,
                    source_node_id: row.get(6)?,
                    role: row.get(7)?,
                    source_field: row.get(8)?,
                    provenance: row.get(9)?,
                    relation: row.get(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let references_truncated = references.len() > 100;
        references.truncate(100);
        drop(statement);
        tx.commit()?;
        Ok(Some(CatalogEntityDetail {
            entity,
            references,
            references_truncated,
        }))
    }

    pub fn catalog_graph(
        &self,
        source: &str,
        project_key: Option<&str>,
        limit: usize,
    ) -> Result<CatalogGraph> {
        self.catalog_graph_tagged(source, project_key, None, limit)
    }

    /// Build the same bounded graph over only chats carrying an exact local tag.
    pub fn catalog_graph_tagged(
        &self,
        source: &str,
        project_key: Option<&str>,
        tag: Option<&str>,
        limit: usize,
    ) -> Result<CatalogGraph> {
        let source_filter = source_filter(source)?;
        validate_project_filter(project_key)?;
        let tag_key = tag.map(crate::store::canonical_tag_key).transpose()?;
        if limit == 0 {
            return Err(Error::Configuration(
                "catalog graph limit must be positive".into(),
            ));
        }
        let limit = limit.min(MAX_CHATS_IN_GRAPH);
        let tx = self.connection.unchecked_transaction()?;
        let index = index_status(&tx)?;
        if !catalog_tables_exist(&tx)? {
            tx.commit()?;
            return Ok(CatalogGraph {
                index,
                stats: CatalogStats::default(),
                ..CatalogGraph::default()
            });
        }
        let stats = read_stats(&tx, source_filter, project_key)?;
        let (nodes, edges, truncated) = read_global_relationship_map(
            &tx,
            source_filter,
            project_key,
            tag_key.as_deref(),
            limit,
        )?;
        let eligible_count =
            read_tagged_chat_count(&tx, source_filter, project_key, tag_key.as_deref())?;
        let unmapped_count = (eligible_count - nodes.len() as i64).max(0);
        tx.commit()?;
        Ok(CatalogGraph {
            index,
            stats,
            truncated,
            unmapped_count,
            focus_id: None,
            nodes,
            edges,
        })
    }

    /// Return a small account-scoped neighborhood around one exact local chat.
    /// Project and keyword relationships connect named chats directly; no
    /// keyword hub nodes are emitted.
    pub fn catalog_neighborhood(
        &self,
        conversation_id: &str,
        source: &str,
        project_key: Option<&str>,
        limit: usize,
    ) -> Result<CatalogGraph> {
        self.catalog_neighborhood_tagged(conversation_id, source, project_key, None, limit)
    }

    /// Return a neighborhood where both the focus chat and every neighbor must
    /// carry the requested local tag.
    pub fn catalog_neighborhood_tagged(
        &self,
        conversation_id: &str,
        source: &str,
        project_key: Option<&str>,
        tag: Option<&str>,
        limit: usize,
    ) -> Result<CatalogGraph> {
        let source_filter = source_filter(source)?;
        validate_project_filter(project_key)?;
        let tag_key = tag.map(crate::store::canonical_tag_key).transpose()?;
        if conversation_id.trim().is_empty() || conversation_id.len() > 256 || limit == 0 {
            return Err(Error::Configuration(
                "invalid catalog neighborhood request".into(),
            ));
        }
        let limit = limit.min(MAX_NEIGHBOR_DEGREE);
        let tx = self.connection.unchecked_transaction()?;
        let index = index_status(&tx)?;
        if !catalog_tables_exist(&tx)? {
            tx.commit()?;
            return Ok(CatalogGraph {
                index,
                stats: CatalogStats::default(),
                ..CatalogGraph::default()
            });
        }
        let stats = read_stats(&tx, source_filter, project_key)?;
        let Some(focus) = read_chat_by_conversation(
            &tx,
            source_filter,
            project_key,
            tag_key.as_deref(),
            conversation_id,
        )?
        else {
            tx.commit()?;
            return Ok(CatalogGraph {
                index,
                stats,
                ..CatalogGraph::default()
            });
        };

        let mut neighbors = Vec::new();
        let mut seen = HashSet::new();
        let mut truncated = false;
        if focus.project_key.is_some() {
            let projects = read_project_neighbors(
                &tx,
                source_filter,
                project_key,
                tag_key.as_deref(),
                &focus,
                limit + 1,
            )?;
            truncated |= projects.len() > limit;
            for neighbor in projects.into_iter().take(limit) {
                seen.insert(neighbor.catalog_id.clone());
                let edge = project_relationship(&focus, &neighbor);
                neighbors.push((neighbor, edge));
            }
        }

        if neighbors.len() < limit {
            let keyword_neighbors = read_keyword_neighbors(
                &tx,
                source_filter,
                project_key,
                tag_key.as_deref(),
                &focus,
                (limit - neighbors.len()) + 1,
            )?;
            truncated |= keyword_neighbors.len() > limit - neighbors.len();
            for candidate in keyword_neighbors {
                if seen.contains(&candidate.catalog_id) {
                    continue;
                }
                let neighbor = read_chat_by_catalog_id(&tx, &candidate.catalog_id)?;
                let Some(neighbor) = neighbor else { continue };
                seen.insert(neighbor.catalog_id.clone());
                neighbors.push((neighbor, keyword_relationship(&focus, &candidate)));
                if neighbors.len() >= limit {
                    break;
                }
            }
        }

        let mut nodes = Vec::with_capacity(neighbors.len() + 1);
        nodes.push(chat_node(&focus));
        let mut edges = Vec::with_capacity(neighbors.len());
        for (neighbor, edge) in neighbors {
            nodes.push(chat_node(&neighbor));
            edges.push(edge);
        }
        let eligible_count =
            read_tagged_chat_count(&tx, source_filter, project_key, tag_key.as_deref())?;
        let unmapped_count = (eligible_count - nodes.len() as i64).max(0);
        tx.commit()?;
        Ok(CatalogGraph {
            index,
            stats,
            truncated,
            unmapped_count,
            focus_id: Some(focus.catalog_id),
            nodes,
            edges,
        })
    }
}

#[derive(Debug)]
struct CandidateRelationship {
    left: usize,
    right: usize,
    edge: CatalogEdge,
}

fn read_global_relationship_map(
    conn: &Connection,
    source_filter: &str,
    project_filter: Option<&str>,
    tag_filter: Option<&str>,
    limit: usize,
) -> Result<(Vec<CatalogNode>, Vec<CatalogEdge>, bool)> {
    let mut chats = read_chat_rows(
        conn,
        source_filter,
        project_filter,
        tag_filter,
        MAX_GLOBAL_GRAPH_CANDIDATES + 1,
    )?;
    let candidate_truncated = chats.len() > MAX_GLOBAL_GRAPH_CANDIDATES;
    chats.truncate(MAX_GLOBAL_GRAPH_CANDIDATES);
    if chats.is_empty() {
        return Ok((Vec::new(), Vec::new(), candidate_truncated));
    }

    let mut by_catalog_id = HashMap::with_capacity(chats.len());
    for (index, chat) in chats.iter().enumerate() {
        by_catalog_id.insert(chat.catalog_id.as_str(), index);
    }
    let mut candidate_edges = Vec::new();
    let mut pairs = HashSet::new();
    let mut truncated = candidate_truncated;

    // Project memberships are explicit. Link each chat to the next few recent
    // chats in that project/account, making a bounded chain instead of a clique.
    let mut project_groups = BTreeMap::<(String, String, String), Vec<usize>>::new();
    for (index, chat) in chats.iter().enumerate() {
        if let Some(project_key) = chat.project_key.as_deref() {
            project_groups
                .entry((
                    chat.provider.clone(),
                    chat.account_scope.clone(),
                    project_key.to_owned(),
                ))
                .or_default()
                .push(index);
        }
    }
    for members in project_groups.values() {
        for left_position in 0..members.len() {
            for offset in 1..=PROJECT_CANDIDATE_NEIGHBORS {
                let Some(&right) = members.get(left_position + offset) else {
                    break;
                };
                let left = members[left_position];
                let pair = (left.min(right), left.max(right));
                if pairs.insert(pair) {
                    push_candidate_edge(
                        &mut candidate_edges,
                        CandidateRelationship {
                            left,
                            right,
                            edge: project_relationship(&chats[left], &chats[right]),
                        },
                        &mut truncated,
                    );
                }
            }
        }
    }

    // Read bounded lexical metadata from the persisted index. A keyword link is
    // considered only within the exact provider/account and requires two terms.
    type TermKey = (String, String, String);
    let project_predicate = project_filter_sql("c", "?1");
    let sql = format!(
        "SELECT c.catalog_id,c.provider,c.account_scope,t.term
         FROM catalog_chats c
         JOIN catalog_edges e ON e.source_id=c.catalog_id AND e.basis='shared_keyword'
         JOIN catalog_terms t ON t.term_id=e.target_id
         WHERE ({source_filter}) AND {project_predicate}
         ORDER BY c.catalog_id,t.term"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([project_filter], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut postings = BTreeMap::<TermKey, Vec<(usize, i64)>>::new();
    let mut chat_terms = vec![Vec::<(TermKey, i64)>::new(); chats.len()];
    let mut chat_count_by_scope = HashMap::<(String, String), i64>::new();
    for chat in &chats {
        *chat_count_by_scope
            .entry((chat.provider.clone(), chat.account_scope.clone()))
            .or_default() += 1;
    }
    for row in rows {
        let (catalog_id, provider, account_scope, term) = row?;
        let Some(&index) = by_catalog_id.get(catalog_id.as_str()) else {
            continue;
        };
        let key = (provider, account_scope, term);
        postings.entry(key.clone()).or_default().push((index, 0));
        chat_terms[index].push((key, 0));
    }
    for terms in &mut chat_terms {
        for (key, document_frequency) in terms {
            let count = postings.get(key).map_or(0, Vec::len);
            *document_frequency = count as i64;
            if count > MAX_KEYWORD_POSTING {
                truncated = true;
            }
        }
    }

    for (left, terms) in chat_terms.iter().enumerate() {
        let mut neighbors = BTreeMap::<usize, Vec<(String, i64)>>::new();
        for (key, document_frequency) in terms {
            if let Some(members) = postings.get(key) {
                if members.len() > MAX_KEYWORD_POSTING {
                    continue;
                }
                for &(right, _) in members {
                    if right > left {
                        neighbors
                            .entry(right)
                            .or_default()
                            .push((key.2.clone(), *document_frequency));
                    }
                }
            }
        }
        for (right, mut evidence) in neighbors {
            evidence.sort_by(|a, b| a.0.cmp(&b.0));
            evidence.dedup_by(|a, b| a.0 == b.0);
            if evidence.len() < 2 {
                continue;
            }
            let pair = (left, right);
            if !pairs.insert(pair) {
                continue;
            }
            let scope_count = chat_count_by_scope
                .get(&(
                    chats[left].provider.clone(),
                    chats[left].account_scope.clone(),
                ))
                .copied()
                .unwrap_or(chats.len() as i64);
            let mut candidate = KeywordCandidate {
                catalog_id: chats[right].catalog_id.clone(),
                updated_at_ms: chats[right].updated_at_ms,
                evidence: evidence.iter().map(|(term, _)| term.clone()).collect(),
                score: 0,
            };
            candidate.score = evidence
                .iter()
                .map(|(_, document_frequency)| {
                    let inverse_frequency =
                        ((scope_count + 1) as f64 / (document_frequency + 1) as f64).ln() + 1.0;
                    (inverse_frequency * 1000.0).round() as i64
                })
                .sum();
            let edge = keyword_relationship(&chats[left], &candidate);
            push_candidate_edge(
                &mut candidate_edges,
                CandidateRelationship { left, right, edge },
                &mut truncated,
            );
        }
    }

    let candidate_edge_count = candidate_edges.len();
    let mut components = relationship_components(chats.len(), &candidate_edges);
    components.sort_by(|left, right| {
        right
            .max_score
            .cmp(&left.max_score)
            .then_with(|| right.node_count.cmp(&left.node_count))
            .then_with(|| left.root.cmp(&right.root))
    });

    let mut selected_edges = Vec::new();
    let mut selected_nodes = Vec::new();
    let mut selected = HashSet::new();
    let mut degrees = vec![0_usize; chats.len()];
    let mut cursors = vec![0_usize; components.len()];
    loop {
        let mut made_progress = false;
        for (component_index, component) in components.iter().enumerate() {
            while let Some(&edge_index) = component.edges.get(cursors[component_index]) {
                cursors[component_index] += 1;
                let edge = &candidate_edges[edge_index];
                let new_nodes = usize::from(!selected.contains(&edge.left))
                    + usize::from(!selected.contains(&edge.right));
                if selected_nodes.len() + new_nodes > limit
                    || degrees[edge.left] >= MAX_NEIGHBOR_DEGREE
                    || degrees[edge.right] >= MAX_NEIGHBOR_DEGREE
                    || selected_edges.len() >= MAX_EDGES_IN_GRAPH
                {
                    truncated = true;
                    continue;
                }
                for index in [edge.left, edge.right] {
                    if selected.insert(index) {
                        selected_nodes.push(index);
                    }
                    degrees[index] += 1;
                }
                selected_edges.push(edge.edge.clone());
                made_progress = true;
                break;
            }
        }
        if !made_progress {
            break;
        }
    }
    if selected_edges.len() < candidate_edge_count || selected_nodes.len() < chats.len() {
        truncated = true;
    }
    let nodes = selected_nodes
        .into_iter()
        .map(|index| chat_node(&chats[index]))
        .collect();
    Ok((nodes, selected_edges, truncated))
}

fn push_candidate_edge(
    edges: &mut Vec<CandidateRelationship>,
    edge: CandidateRelationship,
    truncated: &mut bool,
) {
    if edges.len() >= MAX_GLOBAL_GRAPH_EDGES {
        *truncated = true;
    } else {
        edges.push(edge);
    }
}

#[derive(Debug)]
struct RelationshipComponent {
    root: usize,
    edges: Vec<usize>,
    max_score: i64,
    node_count: usize,
}

fn relationship_components(
    node_count: usize,
    edges: &[CandidateRelationship],
) -> Vec<RelationshipComponent> {
    let mut parent = (0..node_count).collect::<Vec<_>>();
    for edge in edges {
        union_roots(&mut parent, edge.left, edge.right);
    }
    let mut components = BTreeMap::<usize, Vec<usize>>::new();
    for (index, edge) in edges.iter().enumerate() {
        let root = find_root(&mut parent, edge.left);
        components.entry(root).or_default().push(index);
    }
    components
        .into_iter()
        .map(|(root, mut edge_indices)| {
            edge_indices.sort_by(|left, right| {
                edges[*right]
                    .edge
                    .score
                    .cmp(&edges[*left].edge.score)
                    .then_with(|| edges[*left].left.cmp(&edges[*right].left))
                    .then_with(|| edges[*left].right.cmp(&edges[*right].right))
            });
            let mut members = HashSet::new();
            for index in &edge_indices {
                members.insert(edges[*index].left);
                members.insert(edges[*index].right);
            }
            let max_score = edge_indices
                .first()
                .and_then(|index| edges[*index].edge.score)
                .unwrap_or_default();
            RelationshipComponent {
                root,
                edges: edge_indices,
                max_score,
                node_count: members.len(),
            }
        })
        .collect()
}

fn find_root(parent: &mut [usize], node: usize) -> usize {
    if parent[node] != node {
        let root = find_root(parent, parent[node]);
        parent[node] = root;
    }
    parent[node]
}

fn union_roots(parent: &mut [usize], left: usize, right: usize) {
    let left_root = find_root(parent, left);
    let right_root = find_root(parent, right);
    if left_root != right_root {
        let root = left_root.min(right_root);
        let child = left_root.max(right_root);
        parent[child] = root;
    }
}

fn read_chat_rows(
    conn: &Connection,
    source_filter: &str,
    project_key: Option<&str>,
    tag_key: Option<&str>,
    limit: usize,
) -> Result<Vec<IndexedChat>> {
    let project_filter = project_filter_sql("c", "?1");
    let sql = format!(
        "SELECT catalog_id,conversation_id,provider,account_scope,source_id,title,updated_at_ms,project_key,project_label,project_label_source,graph_node_count,message_count,branch_count,source_time,observed_ms,parser,completeness,sample_bytes
         FROM catalog_chats c WHERE ({source_filter}) AND {project_filter}
           AND (?2 IS NULL OR EXISTS(SELECT 1 FROM target_tags tt JOIN tags t ON t.name_key=tt.name_key WHERE tt.target_id=c.conversation_id AND tt.target_kind='conversation' AND t.name_key=?2))
         ORDER BY c.updated_at_ms DESC NULLS LAST,catalog_id LIMIT ?3"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        params![project_key, tag_key, limit as i64],
        indexed_chat_from_row,
    )?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn read_chat_by_conversation(
    conn: &Connection,
    source_filter: &str,
    project_key: Option<&str>,
    tag_key: Option<&str>,
    conversation_id: &str,
) -> Result<Option<IndexedChat>> {
    let project_filter = project_filter_sql("c", "?2");
    let sql = format!(
        "SELECT catalog_id,conversation_id,provider,account_scope,source_id,title,updated_at_ms,project_key,project_label,project_label_source,graph_node_count,message_count,branch_count,source_time,observed_ms,parser,completeness,sample_bytes
         FROM catalog_chats c WHERE c.conversation_id=?1 AND ({source_filter}) AND {project_filter}
           AND (?3 IS NULL OR EXISTS(SELECT 1 FROM target_tags tt JOIN tags t ON t.name_key=tt.name_key WHERE tt.target_id=c.conversation_id AND tt.target_kind='conversation' AND t.name_key=?3)) LIMIT 1"
    );
    Ok(conn
        .query_row(
            &sql,
            params![conversation_id, project_key, tag_key],
            indexed_chat_from_row,
        )
        .optional()?)
}

fn read_chat_by_catalog_id(conn: &Connection, catalog_id: &str) -> Result<Option<IndexedChat>> {
    Ok(conn
        .query_row(
            "SELECT catalog_id,conversation_id,provider,account_scope,source_id,title,updated_at_ms,project_key,project_label,project_label_source,graph_node_count,message_count,branch_count,source_time,observed_ms,parser,completeness,sample_bytes
             FROM catalog_chats WHERE catalog_id=?1",
            params![catalog_id],
            indexed_chat_from_row,
        )
        .optional()?)
}

fn read_project_neighbors(
    conn: &Connection,
    source_filter: &str,
    project_filter: Option<&str>,
    tag_filter: Option<&str>,
    focus: &IndexedChat,
    limit: usize,
) -> Result<Vec<IndexedChat>> {
    let Some(project_key) = focus.project_key.as_deref() else {
        return Ok(Vec::new());
    };
    let sql = format!(
        "SELECT catalog_id,conversation_id,provider,account_scope,source_id,title,updated_at_ms,project_key,project_label,project_label_source,graph_node_count,message_count,branch_count,source_time,observed_ms,parser,completeness,sample_bytes
         FROM catalog_chats c WHERE c.project_key=?1 AND c.catalog_id<>?2 AND c.provider=?3 AND c.account_scope=?4
           AND ({source_filter}) AND (?5 IS NULL OR c.project_key=?5)
           AND (?6 IS NULL OR EXISTS(SELECT 1 FROM target_tags tt JOIN tags t ON t.name_key=tt.name_key WHERE tt.target_id=c.conversation_id AND tt.target_kind='conversation' AND t.name_key=?6))
         ORDER BY c.updated_at_ms DESC NULLS LAST,c.catalog_id LIMIT ?7"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        params![
            project_key,
            focus.catalog_id,
            focus.provider,
            focus.account_scope,
            project_filter,
            tag_filter,
            limit as i64,
        ],
        indexed_chat_from_row,
    )?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

#[derive(Debug)]
struct KeywordCandidate {
    catalog_id: String,
    updated_at_ms: Option<i64>,
    evidence: Vec<String>,
    score: i64,
}

fn read_keyword_neighbors(
    conn: &Connection,
    source_filter: &str,
    project_filter: Option<&str>,
    tag_filter: Option<&str>,
    focus: &IndexedChat,
    limit: usize,
) -> Result<Vec<KeywordCandidate>> {
    let scope_project_filter = project_filter_sql("c", "?3");
    let candidate_project_filter = project_filter_sql("c", "?5");
    let evidence_project_filter = project_filter_sql("c2", "?5");
    let scope_sql = format!(
        "SELECT count(*) FROM catalog_chats c WHERE provider=?1 AND account_scope=?2 AND {scope_project_filter}
           AND (?4 IS NULL OR EXISTS(SELECT 1 FROM target_tags tt JOIN tags tg ON tg.name_key=tt.name_key WHERE tt.target_id=c.conversation_id AND tt.target_kind='conversation' AND tg.name_key=?4))"
    );
    let scope_chat_count: i64 = conn.query_row(
        &scope_sql,
        params![
            focus.provider,
            focus.account_scope,
            project_filter,
            tag_filter
        ],
        |row| row.get(0),
    )?;
    let sql = format!(
        "SELECT c.catalog_id,c.updated_at_ms,group_concat(t.term || ':' || (
             SELECT count(DISTINCT e2.source_id) FROM catalog_edges e2 JOIN catalog_chats c2 ON c2.catalog_id=e2.source_id
             WHERE e2.target_id=t.term_id AND e2.basis='shared_keyword' AND c2.provider=?3 AND c2.account_scope=?4
               AND {evidence_project_filter}
               AND (?6 IS NULL OR EXISTS(SELECT 1 FROM target_tags tt JOIN tags tg ON tg.name_key=tt.name_key WHERE tt.target_id=c2.conversation_id AND tt.target_kind='conversation' AND tg.name_key=?6))
         ),char(31))
         FROM catalog_edges f
         JOIN catalog_terms t ON t.term_id=f.target_id
         JOIN catalog_edges e ON e.target_id=t.term_id AND e.basis='shared_keyword'
         JOIN catalog_chats c ON c.catalog_id=e.source_id
         WHERE f.source_id=?1 AND f.basis='shared_keyword' AND c.catalog_id<>?2
           AND c.provider=?3 AND c.account_scope=?4 AND ({source_filter})
           AND {candidate_project_filter}
           AND (?6 IS NULL OR EXISTS(SELECT 1 FROM target_tags tt JOIN tags tg ON tg.name_key=tt.name_key WHERE tt.target_id=c.conversation_id AND tt.target_kind='conversation' AND tg.name_key=?6))
         GROUP BY c.catalog_id
         HAVING count(DISTINCT t.term_id)>=2
         ORDER BY sum(1000000.0 / t.chat_count) DESC,c.updated_at_ms DESC NULLS LAST,c.catalog_id
         LIMIT ?7"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        params![
            focus.catalog_id,
            focus.catalog_id,
            focus.provider,
            focus.account_scope,
            project_filter,
            tag_filter,
            KEYWORD_CANDIDATE_SCAN as i64,
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let mut candidates = rows
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(catalog_id, updated_at_ms, encoded_terms)| {
            let mut evidence = Vec::new();
            let mut score = 0_i64;
            for item in encoded_terms.split('\u{1f}') {
                let Some((term, count)) = item.rsplit_once(':') else {
                    continue;
                };
                let Ok(document_frequency) = count.parse::<i64>() else {
                    continue;
                };
                if document_frequency < 2 || term.is_empty() {
                    continue;
                }
                let inverse_frequency =
                    ((scope_chat_count + 1) as f64 / (document_frequency + 1) as f64).ln() + 1.0;
                score += (inverse_frequency * 1000.0).round() as i64;
                evidence.push(term.to_owned());
            }
            evidence.sort();
            evidence.dedup();
            KeywordCandidate {
                catalog_id,
                updated_at_ms,
                evidence,
                score,
            }
        })
        .collect::<Vec<_>>();
    candidates.retain(|candidate| candidate.evidence.len() >= 2);
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| left.catalog_id.cmp(&right.catalog_id))
    });
    candidates.truncate(limit);
    Ok(candidates)
}

fn indexed_chat_from_row(row: &Row<'_>) -> rusqlite::Result<IndexedChat> {
    Ok(IndexedChat {
        catalog_id: row.get(0)?,
        conversation_id: row.get(1)?,
        provider: row.get(2)?,
        account_scope: row.get(3)?,
        source_id: row.get(4)?,
        title: row.get(5)?,
        updated_at_ms: row.get(6)?,
        project_key: row.get(7)?,
        project_label: row.get(8)?,
        project_label_source: row.get(9)?,
        graph_node_count: row.get(10)?,
        message_count: row.get(11)?,
        branch_count: row.get(12)?,
        source_time: row.get(13)?,
        observed_ms: row.get(14)?,
        parser: row.get(15)?,
        completeness: row.get(16)?,
        sample_bytes: row.get(17)?,
    })
}

fn chat_node(chat: &IndexedChat) -> CatalogNode {
    CatalogNode {
        id: chat.catalog_id.clone(),
        kind: "chat".into(),
        label: if chat.title.trim().is_empty() {
            "Untitled chat".into()
        } else {
            chat.title.clone()
        },
        label_source: Some("conversation".into()),
        provider: Some(chat.provider.clone()),
        account_scope: Some(chat.account_scope.clone()),
        conversation_id: Some(chat.conversation_id.clone()),
        source_id: Some(chat.source_id.clone()),
        updated_at_ms: chat.updated_at_ms,
        graph_node_count: Some(chat.graph_node_count),
        message_count: Some(chat.message_count),
        branch_count: Some(chat.branch_count),
        source_time: chat.source_time.clone(),
        observed_ms: chat.observed_ms,
        parser: chat.parser.clone(),
        completeness: chat.completeness.clone(),
        cached_archive: Some(chat.completeness.as_deref() == Some("cached_transcript")),
        sample_bytes: Some(chat.sample_bytes),
        ..CatalogNode::default()
    }
}

fn project_relationship(left: &IndexedChat, right: &IndexedChat) -> CatalogEdge {
    let label = left
        .project_label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or("Project name pending");
    let (reason, evidence) = if left.project_label_source.as_deref() == Some("unknown") {
        (
            "Same project (name pending)".to_owned(),
            vec!["explicit project membership".to_owned()],
        )
    } else {
        (format!("Same project: {label}"), vec![label.to_owned()])
    };
    CatalogEdge {
        source: left.catalog_id.clone(),
        target: right.catalog_id.clone(),
        basis: "project".into(),
        reason: Some(reason),
        evidence,
        score: Some(1_000_000),
    }
}

fn project_catalog_id(project_key: &str) -> String {
    format!("project:{}", serde_json::json!([project_key]))
}

fn keyword_relationship(focus: &IndexedChat, candidate: &KeywordCandidate) -> CatalogEdge {
    let evidence = candidate
        .evidence
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>();
    CatalogEdge {
        source: focus.catalog_id.clone(),
        target: candidate.catalog_id.clone(),
        basis: "shared_keyword".into(),
        reason: Some(format!(
            "Shares {} indexed keywords",
            candidate.evidence.len()
        )),
        evidence,
        score: Some(candidate.score),
    }
}

fn source_filter(source: &str) -> Result<&'static str> {
    match source {
        "chatgpt" => Ok("c.provider IN ('chatgpt','paseo-chatgpt')"),
        "codex" => Ok("c.provider LIKE 'codex%' OR c.provider LIKE 'paseo-codex%'"),
        "all" => Ok("1=1"),
        _ => Err(Error::Configuration("invalid catalog source".into())),
    }
}

fn read_tagged_chat_count(
    conn: &Connection,
    source_filter: &str,
    project_key: Option<&str>,
    tag_key: Option<&str>,
) -> Result<i64> {
    let project_filter = project_filter_sql("c", "?1");
    let sql = format!(
        "SELECT count(*) FROM catalog_chats c WHERE ({source_filter}) AND {project_filter} AND (?2 IS NULL OR EXISTS(SELECT 1 FROM target_tags tt JOIN tags t ON t.name_key=tt.name_key WHERE tt.target_id=c.conversation_id AND tt.target_kind='conversation' AND t.name_key=?2))"
    );
    Ok(conn.query_row(&sql, params![project_key, tag_key], |row| row.get(0))?)
}

fn validate_project_filter(project_key: Option<&str>) -> Result<()> {
    if project_key.is_some_and(|key| key.trim().is_empty() || key.len() > 1024) {
        return Err(Error::Configuration("invalid catalog project key".into()));
    }
    Ok(())
}

fn current_revision(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT revision FROM library_state WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

fn read_chat_batch(tx: &Transaction<'_>, after_id: Option<&str>) -> Result<Vec<SourceChat>> {
    let mut statement = tx.prepare(
        "SELECT c.id,c.provider,c.account_scope,c.source_id,
                substr(CAST(c.title AS BLOB),1,1024),c.updated_at_ms,
                cp.project_key,p.local_name,p.title,
                (SELECT count(*) FROM graph_nodes g WHERE g.conversation_id=c.id),
                (SELECT count(*) FROM graph_nodes g WHERE g.conversation_id=c.id AND g.message_id IS NOT NULL),
                (SELECT count(*) FROM (SELECT g.parent_id FROM graph_nodes g
                  WHERE g.conversation_id=c.id AND g.parent_id IS NOT NULL
                    AND EXISTS(SELECT 1 FROM graph_nodes parent WHERE parent.conversation_id=c.id AND parent.source_node_id=g.parent_id)
                  GROUP BY g.parent_id HAVING count(*)>1)),
                (SELECT substr(source_time,1,256) FROM observations o WHERE o.conversation_id=c.id ORDER BY o.id DESC LIMIT 1),
                (SELECT observed_ms FROM observations o WHERE o.conversation_id=c.id ORDER BY o.id DESC LIMIT 1),
                (SELECT substr(parser,1,128) FROM observations o WHERE o.conversation_id=c.id ORDER BY o.id DESC LIMIT 1),
                (SELECT substr(completeness,1,128) FROM observations o WHERE o.conversation_id=c.id ORDER BY o.id DESC LIMIT 1)
         FROM conversations c
         LEFT JOIN conversation_projects cp ON cp.conversation_id=c.id
         LEFT JOIN projects p ON p.project_key=cp.project_key
         WHERE (?1 IS NULL OR c.id>?1)
         ORDER BY c.id LIMIT ?2",
    )?;
    let rows = statement.query_map(params![after_id, REBUILD_BATCH_CHATS as i64], |row| {
        let completeness: Option<String> = row.get(15)?;
        let archive = completeness.as_deref() == Some("cached_transcript");
        Ok(SourceChat {
            conversation_id: row.get(0)?,
            provider: row.get(1)?,
            account_scope: row.get(2)?,
            source_id: row.get(3)?,
            title_bytes: row.get(4)?,
            updated_at_ms: row.get(5)?,
            project_key: row.get(6)?,
            project_local_name: row.get(7)?,
            project_title: row.get(8)?,
            graph_node_count: row.get(9)?,
            message_count: if archive { 0 } else { row.get(10)? },
            branch_count: if archive { 0 } else { row.get(11)? },
            source_time: row.get(12)?,
            observed_ms: row.get(13)?,
            parser: row.get(14)?,
            completeness,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn catalog_tables_exist(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='catalog_meta')",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

fn entity_tables_exist(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='catalog_entities')",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

fn apply_saved_artifact_state(conn: &Connection, entity: &mut CatalogEntity) -> Result<()> {
    if entity.kind != CatalogEntityKind::FileArtifact {
        return Ok(());
    }
    let table_exists = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='artifact_contents')", [], |row| row.get::<_,i64>(0))? != 0;
    if !table_exists {
        return Ok(());
    }
    let saved = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM artifact_contents WHERE entity_id=?1)",
        [&entity.id],
        |row| row.get::<_, i64>(0),
    )? != 0;
    if saved {
        entity.content_saved = true;
        entity.artifact_state = Some("saved_locally".into());
    }
    Ok(())
}

fn entity_from_row(row: &Row<'_>) -> rusqlite::Result<CatalogEntity> {
    Ok(CatalogEntity {
        id: row.get(0)?,
        kind: CatalogEntityKind::parse(&row.get::<_, String>(1)?).unwrap_or_default(),
        label: row.get(2)?,
        canonical_url: row.get(3)?,
        domain: row.get(4)?,
        repository_id: row.get(5)?,
        filename: row.get(6)?,
        mime_type: row.get(7)?,
        provider_file_id: row.get(8)?,
        canonical_locator: row.get(9)?,
        artifact_state: row.get(10)?,
        content_saved: row.get::<_, i64>(11)? != 0,
        reference_count: row.get(12)?,
        produced_reference_count: row.get(13)?,
        attachment_reference_count: row.get(14)?,
        mentioned_reference_count: row.get(15)?,
    })
}

fn validate_entity_filters(
    conversation_id: Option<&str>,
    artifact_state: Option<&str>,
    cursor: Option<&str>,
) -> Result<()> {
    if conversation_id.is_some_and(|v| v.trim().is_empty() || v.len() > 256)
        || cursor.is_some_and(|v| v.trim().is_empty() || v.len() > 256)
        || artifact_state
            .is_some_and(|v| !matches!(v, "link_only" | "metadata_present" | "saved_locally"))
    {
        return Err(Error::Configuration("invalid catalog entity filter".into()));
    }
    Ok(())
}

fn entity_filter_sql(source: &str) -> String {
    let project_filter = project_filter_sql("c", "?1");
    format!(
        "({source}) AND {project_filter} AND (?2 IS NULL OR c.conversation_id=?2) AND (?3 IS NULL OR e.kind=?3) AND (?4 IS NULL OR e.artifact_state=?4) AND (?5 IS NULL OR EXISTS(SELECT 1 FROM target_tags tt JOIN tags t ON t.name_key=tt.name_key WHERE tt.target_id=e.id AND tt.target_kind=CASE WHEN e.kind='file_artifact' THEN 'file' ELSE 'entity' END AND t.name_key=?5)) AND (?6 IS NULL OR instr(lower(e.label),lower(?6))>0 OR instr(lower(coalesce(e.canonical_url,'')),lower(?6))>0 OR instr(lower(coalesce(e.domain,'')),lower(?6))>0 OR instr(lower(coalesce(e.filename,'')),lower(?6))>0) AND (?7 IS NULL OR e.id>?7)"
    )
}

fn project_filter_sql(alias: &str, parameter: &str) -> String {
    format!(
        "({parameter} IS NULL OR ({parameter}='{}' AND {alias}.project_key IS NULL) OR {alias}.project_key={parameter})",
        crate::store::NO_PROJECT_FILTER
    )
}

fn index_status(conn: &Connection) -> Result<CatalogIndexStatus> {
    let library_revision = current_revision(conn)?;
    if !catalog_tables_exist(conn)? {
        return Ok(CatalogIndexStatus {
            index_version: CATALOG_INDEX_VERSION,
            library_revision,
            indexed_revision: None,
            rebuilt_at_ms: None,
            stale: true,
        });
    }
    let stored: Option<(i64, i64, i64)> = conn
        .query_row(
            "SELECT index_version,library_revision,rebuilt_at_ms FROM catalog_meta WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(match stored {
        Some((version, revision, rebuilt_at_ms)) => CatalogIndexStatus {
            index_version: version,
            library_revision,
            indexed_revision: Some(revision),
            rebuilt_at_ms: Some(rebuilt_at_ms),
            stale: version != CATALOG_INDEX_VERSION || revision != library_revision,
        },
        None => CatalogIndexStatus {
            index_version: CATALOG_INDEX_VERSION,
            library_revision,
            indexed_revision: None,
            rebuilt_at_ms: None,
            stale: true,
        },
    })
}

type RawCatalogStatsRow = (
    i64,
    i64,
    i64,
    i64,
    Option<i64>,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
);

fn read_stats(
    conn: &Connection,
    source_filter: &str,
    project_key: Option<&str>,
) -> Result<CatalogStats> {
    let project_filter = project_filter_sql("c", "?1");
    let sql = format!(
        "SELECT count(*),coalesce(sum(c.graph_node_count),0),coalesce(sum(c.message_count),0),coalesce(sum(c.branch_count),0),max(c.updated_at_ms),
                coalesce(sum(c.source_time IS NOT NULL AND trim(c.source_time)<>''),0),
                coalesce(sum(c.parser IS NOT NULL AND trim(c.parser)<>''),0),
                coalesce(sum(c.completeness IS NOT NULL AND trim(c.completeness)<>''),0),
                coalesce(sum(c.completeness='cached_transcript'),0),
                coalesce(sum(c.completeness<>'cached_transcript' AND c.graph_node_count>0),0),
                count(DISTINCT c.project_key)
         FROM catalog_chats c WHERE ({source_filter}) AND {project_filter}"
    );
    let (
        chats,
        graph_nodes,
        messages,
        branch_points,
        latest_activity_ms,
        source_time,
        parser,
        completeness,
        cached_archives,
        full_graphs,
        projects,
    ): RawCatalogStatsRow = conn.query_row(&sql, params![project_key], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
        ))
    })?;
    let shared_keyword_sql = format!(
        "SELECT count(DISTINCT e.target_id) FROM catalog_edges e JOIN catalog_chats c ON c.catalog_id=e.source_id
         WHERE e.basis='shared_keyword' AND ({source_filter}) AND {project_filter}"
    );
    let shared_keywords: i64 =
        conn.query_row(&shared_keyword_sql, params![project_key], |row| row.get(0))?;
    let source_sql = format!(
        "SELECT c.provider,count(*),coalesce(sum(c.message_count),0),coalesce(sum(c.graph_node_count),0),coalesce(sum(c.branch_count),0)
         FROM catalog_chats c WHERE ({source_filter}) AND {project_filter}
         GROUP BY c.provider ORDER BY c.provider"
    );
    let mut statement = conn.prepare(&source_sql)?;
    let source_coverage = statement
        .query_map(params![project_key], |row| {
            Ok(CatalogSourceStats {
                provider: row.get(0)?,
                chats: row.get(1)?,
                messages: row.get(2)?,
                graph_nodes: row.get(3)?,
                branch_points: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let entity_counts = if entity_tables_exist(conn)? {
        entity_counts(conn, source_filter, project_key)?
    } else {
        (0, 0, 0, 0, 0, 0, 0)
    };
    Ok(CatalogStats {
        chats,
        projects,
        shared_keywords,
        messages,
        graph_nodes,
        branch_points,
        entities: entity_counts.0,
        repositories: entity_counts.1,
        pull_requests: entity_counts.2,
        issues: entity_counts.3,
        domains: entity_counts.4,
        links: entity_counts.5,
        file_artifacts: entity_counts.6,
        latest_activity_ms,
        source_coverage,
        provenance: CatalogProvenanceCoverage {
            chats_with_source_time: source_time,
            chats_with_parser: parser,
            chats_with_completeness: completeness,
            cached_archives,
            full_graphs,
        },
    })
}

fn entity_counts(
    conn: &Connection,
    source_filter: &str,
    project_key: Option<&str>,
) -> Result<(i64, i64, i64, i64, i64, i64, i64)> {
    let project_filter = project_filter_sql("c", "?1");
    let sql = format!(
        "SELECT e.kind,count(DISTINCT e.id) FROM catalog_entities e JOIN catalog_entity_refs r ON r.entity_id=e.id JOIN catalog_chats c ON c.conversation_id=r.conversation_id WHERE ({source_filter}) AND {project_filter} GROUP BY e.kind"
    );
    let mut counts = HashMap::new();
    let mut statement = conn.prepare(&sql)?;
    for row in statement.query_map([project_key], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (kind, count) = row?;
        counts.insert(kind, count);
    }
    let get = |kind: &str| counts.get(kind).copied().unwrap_or_default();
    Ok((
        get("repository")
            + get("pull_request")
            + get("issue")
            + get("domain")
            + get("link")
            + get("file_artifact"),
        get("repository"),
        get("pull_request"),
        get("issue"),
        get("domain"),
        get("link"),
        get("file_artifact"),
    ))
}

fn read_bounded_sample(tx: &Transaction<'_>, conversation_id: &str) -> Result<(String, usize)> {
    let mut statement = tx.prepare(
        "SELECT substr(CAST(content AS BLOB),1,?2) FROM graph_nodes
         WHERE conversation_id=?1 AND length(content)>0 ORDER BY source_node_id LIMIT ?3",
    )?;
    let chunks = statement.query_map(
        params![
            conversation_id,
            SAMPLE_CHUNK_BYTES as i64,
            MAX_SAMPLE_ROWS as i64
        ],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    let mut sample = String::new();
    let mut byte_count = 0_usize;
    for chunk in chunks {
        let chunk = chunk?;
        if byte_count >= MAX_SAMPLE_BYTES {
            break;
        }
        let remaining = MAX_SAMPLE_BYTES.saturating_sub(byte_count);
        let bytes = &chunk[..chunk.len().min(remaining)];
        sample.push_str(&String::from_utf8_lossy(bytes));
        byte_count += bytes.len();
    }
    Ok((sample, byte_count))
}

#[derive(Clone)]
struct EntityOccurrence {
    kind: CatalogEntityKind,
    label: String,
    canonical_url: Option<String>,
    domain: Option<String>,
    repository_id: Option<String>,
    filename: Option<String>,
    mime_type: Option<String>,
    provider_file_id: Option<String>,
    canonical_locator: Option<String>,
    artifact_state: Option<String>,
    content_saved: bool,
    source_field: String,
    provenance: String,
    relation: String,
    identity: String,
}

fn index_chat_entities(tx: &Transaction<'_>, chat: &SourceChat, title: &str) -> Result<()> {
    let mut bytes_scanned = 0_usize;
    let mut occurrence_count = 0_usize;
    let mut truncated = false;
    let mut statement = tx.prepare(
        "SELECT source_node_id,message_id,source_message_id,role,substr(CAST(coalesce(content,'') AS BLOB),1,?2),substr(CAST(coalesce(parts_json,'') AS BLOB),1,?2),length(CAST(coalesce(content,'') AS BLOB)),length(CAST(coalesce(parts_json,'') AS BLOB)) FROM graph_nodes WHERE conversation_id=?1 ORDER BY source_node_id",
    )?;
    let rows = statement.query_map(
        params![&chat.conversation_id, MAX_ENTITY_SCAN_PER_FIELD as i64],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?.unwrap_or_default(),
                row.get::<_, Option<Vec<u8>>>(5)?.unwrap_or_default(),
                row.get::<_, Option<usize>>(6)?.unwrap_or_default(),
                row.get::<_, Option<usize>>(7)?.unwrap_or_default(),
            ))
        },
    )?;
    for row in rows {
        let (node_id, message_id, source_message_id, role, content, parts, content_len, parts_len) =
            row?;
        if content_len > MAX_ENTITY_SCAN_PER_FIELD || parts_len > MAX_ENTITY_SCAN_PER_FIELD {
            truncated = true;
        }
        let mut node_occurrences = Vec::new();
        let mut fields = vec![
            ("graph_nodes.content", content),
            ("graph_nodes.parts_json", parts),
        ];
        for (source_field, raw) in fields.drain(..) {
            if bytes_scanned >= MAX_ENTITY_SCAN_PER_CHAT {
                truncated = true;
                break;
            }
            let remaining = MAX_ENTITY_SCAN_PER_CHAT - bytes_scanned;
            let take = raw.len().min(MAX_ENTITY_SCAN_PER_FIELD).min(remaining);
            if take < raw.len() {
                truncated = true;
            }
            let text = String::from_utf8_lossy(&raw[..take]);
            bytes_scanned += take;
            let provenance = if source_field.ends_with("parts_json") {
                "normalized_parts"
            } else {
                "normalized_text"
            };
            for url in extract_explicit_links(&text) {
                append_link_occurrences(
                    &mut node_occurrences,
                    &url,
                    &chat.provider,
                    &chat.account_scope,
                    source_field,
                    provenance,
                );
            }
            if source_field.ends_with("parts_json") {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw[..take]) {
                    extract_structured_artifacts(
                        &value,
                        &mut node_occurrences,
                        source_field,
                        provenance,
                    );
                }
            }
            if node_occurrences.len() >= MAX_ENTITY_OCCURRENCES_PER_CHAT {
                node_occurrences.truncate(MAX_ENTITY_OCCURRENCES_PER_CHAT);
                truncated = true;
            }
        }
        if occurrence_count + node_occurrences.len() > MAX_ENTITY_OCCURRENCES_PER_CHAT {
            node_occurrences
                .truncate(MAX_ENTITY_OCCURRENCES_PER_CHAT.saturating_sub(occurrence_count));
            truncated = true;
        }
        occurrence_count += node_occurrences.len();
        persist_occurrences(
            tx,
            chat,
            title,
            &node_id,
            message_id.as_deref(),
            source_message_id.as_deref(),
            role.as_deref(),
            node_occurrences,
        )?;
        if bytes_scanned >= MAX_ENTITY_SCAN_PER_CHAT
            || occurrence_count >= MAX_ENTITY_OCCURRENCES_PER_CHAT
        {
            truncated = true;
            break;
        }
    }
    drop(statement);
    if bytes_scanned < MAX_ENTITY_SCAN_PER_CHAT
        && occurrence_count < MAX_ENTITY_OCCURRENCES_PER_CHAT
    {
        let raw: Option<(i64,Vec<u8>,usize)> = tx.query_row(
            "SELECT id,substr(CAST(raw_payload AS BLOB),1,?2),length(CAST(raw_payload AS BLOB)) FROM observations WHERE conversation_id=?1 ORDER BY id DESC LIMIT 1",
            params![&chat.conversation_id,MAX_ENTITY_SCAN_PER_FIELD as i64],
            |row| Ok((row.get(0)?,row.get::<_,Option<Vec<u8>>>(1)?.unwrap_or_default(),row.get::<_,Option<usize>>(2)?.unwrap_or_default())),
        ).optional()?;
        if let Some((observation_id, raw, raw_len)) = raw {
            if raw_len > raw.len() {
                truncated = true;
            }
            let take = raw.len().min(MAX_ENTITY_SCAN_PER_CHAT - bytes_scanned);
            let text = String::from_utf8_lossy(&raw[..take]);
            let mut raw_occurrences = Vec::new();
            for url in extract_explicit_links(&text) {
                append_link_occurrences(
                    &mut raw_occurrences,
                    &url,
                    &chat.provider,
                    &chat.account_scope,
                    "observations.raw_payload",
                    "observation_metadata",
                );
            }
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw[..take]) {
                extract_structured_artifacts(
                    &value,
                    &mut raw_occurrences,
                    "observations.raw_payload",
                    "observation_metadata",
                );
            }
            if raw_occurrences.len() + occurrence_count > MAX_ENTITY_OCCURRENCES_PER_CHAT {
                raw_occurrences
                    .truncate(MAX_ENTITY_OCCURRENCES_PER_CHAT.saturating_sub(occurrence_count));
                truncated = true;
            }
            persist_occurrences(
                tx,
                chat,
                title,
                &format!("observation:{observation_id}"),
                None,
                None,
                None,
                raw_occurrences,
            )?;
        }
    }
    tx.execute(
        "INSERT INTO catalog_scan_coverage(conversation_id,truncated) VALUES(?1,?2)",
        params![chat.conversation_id, i64::from(truncated)],
    )?;
    Ok(())
}

fn extract_explicit_links(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for token in text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '<' | '>' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}'
            )
    }) {
        let candidate =
            token.trim_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '`'));
        if (candidate.starts_with("https://")
            || candidate.starts_with("http://")
            || candidate.starts_with("sandbox:/mnt/data/"))
            && candidate.len() <= 4096
            && !found.iter().any(|v| v == candidate)
        {
            found.push(candidate.to_owned());
        }
    }
    found
}

fn sanitize_http_url(input: &str) -> Option<(String, String, String)> {
    let rest = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))?;
    let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty()
        || authority.contains('@')
        || authority.contains('?')
        || authority.contains('#')
    {
        return None;
    }
    let host = authority
        .split(':')
        .next()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 || !host.contains('.') {
        return None;
    }
    let path = tail.split(['?', '#']).next().unwrap_or("");
    let path = format!("/{}", path.trim_start_matches('/'));
    let scheme = if input.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    let canonical = format!("{scheme}://{host}{path}");
    Some((canonical, host, path))
}

fn append_link_occurrences(
    out: &mut Vec<EntityOccurrence>,
    raw: &str,
    provider: &str,
    scope: &str,
    source_field: &str,
    provenance: &str,
) {
    if raw.starts_with("sandbox:/mnt/data/") {
        let path = raw
            .strip_prefix("sandbox:")
            .unwrap_or(raw)
            .split(['?', '#'])
            .next()
            .unwrap_or("");
        let filename = path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("Generated file")
            .to_owned();
        let locator = format!("sandbox:{path}");
        out.push(EntityOccurrence {
            kind: CatalogEntityKind::FileArtifact,
            label: filename.clone(),
            canonical_url: None,
            domain: None,
            repository_id: None,
            filename: Some(filename),
            mime_type: None,
            provider_file_id: None,
            canonical_locator: Some(locator.clone()),
            artifact_state: Some("link_only".into()),
            content_saved: false,
            source_field: source_field.into(),
            provenance: provenance.into(),
            relation: "mentions".into(),
            identity: format!("file:{}:{}", provider, scope),
        });
        return;
    }
    let Some((canonical, host, path)) = sanitize_http_url(raw) else {
        return;
    };
    let repo = classify_code_host(&host, &path);
    let (kind, label, repo_path, repo_entity_id) = if let Some((kind, label, repo_path)) = repo {
        let repository_id = entity_id(&[provider, scope, "repository", &repo_path]);
        if kind != CatalogEntityKind::Repository {
            let repo_url = format!("https://{host}/{repo_path}");
            out.push(EntityOccurrence {
                kind: CatalogEntityKind::Repository,
                label: repo_path.clone(),
                canonical_url: Some(repo_url),
                domain: Some(host.clone()),
                repository_id: None,
                filename: None,
                mime_type: None,
                provider_file_id: None,
                canonical_locator: None,
                artifact_state: None,
                content_saved: false,
                source_field: source_field.into(),
                provenance: provenance.into(),
                relation: "mentions".into(),
                identity: format!("id:{repository_id}"),
            });
        }
        (kind, label, repo_path, Some(repository_id))
    } else {
        (
            CatalogEntityKind::Link,
            canonical.clone(),
            String::new(),
            None,
        )
    };
    let entity_identity = match kind {
        CatalogEntityKind::Repository => entity_id(&[provider, scope, "repository", &repo_path]),
        CatalogEntityKind::PullRequest | CatalogEntityKind::Issue => {
            entity_id(&[provider, scope, kind.as_str(), &canonical])
        }
        _ => entity_id(&[provider, scope, "link", &canonical]),
    };
    out.push(EntityOccurrence {
        kind,
        label,
        canonical_url: Some(canonical),
        domain: Some(host.clone()),
        repository_id: repo_entity_id,
        filename: None,
        mime_type: None,
        provider_file_id: None,
        canonical_locator: None,
        artifact_state: None,
        content_saved: false,
        source_field: source_field.into(),
        provenance: provenance.into(),
        relation: "mentions".into(),
        identity: format!("id:{entity_identity}"),
    });
    let domain_id = entity_id(&[provider, scope, "domain", &host]);
    out.push(EntityOccurrence {
        kind: CatalogEntityKind::Domain,
        label: host.clone(),
        canonical_url: Some(format!("https://{host}/")),
        domain: Some(host),
        repository_id: None,
        filename: None,
        mime_type: None,
        provider_file_id: None,
        canonical_locator: None,
        artifact_state: None,
        content_saved: false,
        source_field: source_field.into(),
        provenance: provenance.into(),
        relation: "mentions".into(),
        identity: format!("id:{domain_id}"),
    });
}

fn classify_code_host(host: &str, path: &str) -> Option<(CatalogEntityKind, String, String)> {
    let segments = path
        .trim_matches('/')
        .split('/')
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    if host == "github.com" && segments.len() >= 2 {
        let repo_path = format!("{}/{}", segments[0], segments[1].trim_end_matches(".git"));
        if segments.len() >= 3
            && segments[2] == "pull"
            && segments
                .get(3)
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
        {
            return Some((
                CatalogEntityKind::PullRequest,
                format!("{repo_path} #{}", segments[3]),
                repo_path,
            ));
        }
        if segments.len() >= 3
            && segments[2] == "issues"
            && segments
                .get(3)
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
        {
            return Some((
                CatalogEntityKind::Issue,
                format!("{repo_path} #{}", segments[3]),
                repo_path,
            ));
        }
        return Some((CatalogEntityKind::Repository, repo_path.clone(), repo_path));
    }
    if host.ends_with("gitlab.com") && !segments.is_empty() {
        if let Some(i) = segments.iter().position(|s| *s == "-") {
            let project = segments[..i].join("/");
            if segments.get(i + 1) == Some(&"merge_requests")
                && segments
                    .get(i + 2)
                    .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
            {
                return Some((
                    CatalogEntityKind::PullRequest,
                    format!("{project} !{}", segments[i + 2]),
                    project,
                ));
            }
            if segments.get(i + 1) == Some(&"issues")
                && segments
                    .get(i + 2)
                    .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
            {
                return Some((
                    CatalogEntityKind::Issue,
                    format!("{project} #{}", segments[i + 2]),
                    project,
                ));
            }
            return Some((CatalogEntityKind::Repository, project.clone(), project));
        }
        let project = segments.join("/");
        return Some((CatalogEntityKind::Repository, project.clone(), project));
    }
    None
}

fn extract_structured_artifacts(
    value: &serde_json::Value,
    out: &mut Vec<EntityOccurrence>,
    source_field: &str,
    provenance: &str,
) {
    match value {
        serde_json::Value::Object(map) => {
            let string = |keys: &[&str]| {
                keys.iter()
                    .find_map(|k| map.get(*k).and_then(|v| v.as_str()))
                    .filter(|s| !s.trim().is_empty() && s.len() <= 4096)
            };
            let filename = string(&["filename", "file_name"]);
            let file_id = string(&["file_id", "fileId", "asset_pointer", "assetPointer"]);
            let locator = string(&["download_url", "downloadUrl", "url", "href"]);
            let mime = string(&["mime_type", "mimeType", "content_type"]);
            if let Some(filename) = filename.filter(|_| file_id.is_some() || locator.is_some()) {
                let filename = filename.to_owned();
                let provider_file_id = file_id.map(str::to_owned);
                let locator = locator.and_then(|v| {
                    if v.starts_with("sandbox:/mnt/data/") {
                        Some(v.split(['?', '#']).next().unwrap_or(v).to_owned())
                    } else {
                        sanitize_http_url(v).map(|v| v.0)
                    }
                });
                let identity = format!(
                    "file:{}",
                    provider_file_id
                        .as_deref()
                        .unwrap_or(locator.as_deref().unwrap_or(&filename))
                );
                out.push(EntityOccurrence {
                    kind: CatalogEntityKind::FileArtifact,
                    label: filename.clone(),
                    canonical_url: None,
                    domain: None,
                    repository_id: None,
                    filename: Some(filename),
                    mime_type: mime.map(str::to_owned),
                    provider_file_id,
                    canonical_locator: locator,
                    artifact_state: Some("metadata_present".into()),
                    content_saved: false,
                    source_field: source_field.into(),
                    provenance: provenance.into(),
                    relation: "attaches".into(),
                    identity,
                });
            }
            for child in map.values() {
                extract_structured_artifacts(child, out, source_field, provenance);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                extract_structured_artifacts(child, out, source_field, provenance);
            }
        }
        _ => {}
    }
}

fn entity_id(parts: &[&str]) -> String {
    let encoded = serde_json::to_vec(parts).unwrap_or_default();
    let digest = ring::digest::digest(&ring::digest::SHA256, &encoded);
    let hex = digest
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("entity:{hex}")
}

#[allow(clippy::too_many_arguments)]
fn persist_occurrences(
    tx: &Transaction<'_>,
    chat: &SourceChat,
    title: &str,
    node_id: &str,
    message_id: Option<&str>,
    source_message_id: Option<&str>,
    role: Option<&str>,
    occurrences: Vec<EntityOccurrence>,
) -> Result<()> {
    let mut seen = HashSet::new();
    for occurrence in occurrences {
        let id = if occurrence.identity.starts_with("id:") {
            occurrence.identity[3..].to_owned()
        } else if occurrence.kind == CatalogEntityKind::FileArtifact
            && occurrence.provider_file_id.is_some()
        {
            entity_id(&[
                &chat.provider,
                &chat.account_scope,
                "file_artifact",
                occurrence.provider_file_id.as_deref().unwrap_or_default(),
            ])
        } else {
            let identity = occurrence
                .identity
                .strip_prefix("file:")
                .unwrap_or(&occurrence.identity);
            entity_id(&[
                &chat.provider,
                &chat.account_scope,
                occurrence.kind.as_str(),
                &chat.conversation_id,
                node_id,
                identity,
            ])
        };
        let fingerprint = format!("{id}:{}:{}", occurrence.source_field, occurrence.relation);
        if !seen.insert(fingerprint) {
            continue;
        }
        tx.execute("INSERT INTO catalog_entities(id,provider,account_scope,kind,label,canonical_url,domain,repository_id,filename,mime_type,provider_file_id,canonical_locator,artifact_state,content_saved,reference_count) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,0) ON CONFLICT(id) DO UPDATE SET label=excluded.label,filename=coalesce(catalog_entities.filename,excluded.filename),mime_type=coalesce(catalog_entities.mime_type,excluded.mime_type),provider_file_id=coalesce(catalog_entities.provider_file_id,excluded.provider_file_id),canonical_locator=coalesce(catalog_entities.canonical_locator,excluded.canonical_locator),artifact_state=coalesce(catalog_entities.artifact_state,excluded.artifact_state)",params![id,chat.provider,chat.account_scope,occurrence.kind.as_str(),occurrence.label,occurrence.canonical_url,occurrence.domain,occurrence.repository_id,occurrence.filename,occurrence.mime_type,occurrence.provider_file_id,occurrence.canonical_locator,occurrence.artifact_state,i64::from(occurrence.content_saved)])?;
        let relation = if occurrence.kind == CatalogEntityKind::FileArtifact
            && occurrence.artifact_state.as_deref() == Some("metadata_present")
            && role == Some("assistant")
        {
            "produces"
        } else {
            occurrence.relation.as_str()
        };
        tx.execute("INSERT OR IGNORE INTO catalog_entity_refs(entity_id,conversation_id,source_id,title,provider,message_id,source_message_id,source_node_id,role,source_field,provenance,relation) SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12 WHERE (SELECT count(*) FROM catalog_entity_refs WHERE entity_id=?1)<?13",params![id,chat.conversation_id,chat.source_id,title,chat.provider,message_id,source_message_id,node_id,role,occurrence.source_field,occurrence.provenance,relation,MAX_ENTITY_REFS_PER_ENTITY as i64])?;
        tx.execute("UPDATE catalog_entities SET reference_count=(SELECT count(*) FROM catalog_entity_refs WHERE entity_id=?1) WHERE id=?1",[&id])?;
    }
    Ok(())
}

fn project_label(local: Option<&str>, upstream: Option<&str>) -> (Option<String>, Option<String>) {
    if let Some(value) = local.map(str::trim).filter(|value| !value.is_empty()) {
        (Some(value.to_owned()), Some("local".into()))
    } else if let Some(value) = upstream.map(str::trim).filter(|value| !value.is_empty()) {
        (Some(value.to_owned()), Some("upstream".into()))
    } else {
        (Some("Project name pending".into()), Some("unknown".into()))
    }
}

fn chat_catalog_id(provider: &str, account_scope: &str, source_id: &str) -> String {
    format!(
        "chat:{}",
        serde_json::json!([provider, account_scope, source_id])
    )
}

fn keyword_catalog_id(provider: &str, account_scope: &str, term: &str) -> String {
    format!(
        "keyword:{}",
        serde_json::json!([provider, account_scope, term])
    )
}

fn choose_keywords(title: &str, sample: &str) -> Vec<String> {
    let mut scores = BTreeMap::<String, u16>::new();
    score_text(title, 4, &mut scores);
    score_text(sample, 1, &mut scores);
    let mut terms = scores.into_iter().collect::<Vec<_>>();
    terms.sort_by(|(left_term, left_score), (right_term, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_term.cmp(right_term))
    });
    terms
        .into_iter()
        .take(MAX_TERMS_PER_CHAT)
        .map(|(term, _)| term)
        .collect()
}

fn score_text(text: &str, weight: u16, scores: &mut BTreeMap<String, u16>) {
    let mut seen = HashSet::<String>::new();
    for raw in text.split(|ch: char| !ch.is_alphanumeric()) {
        let term = raw.to_lowercase();
        let char_count = term.chars().count();
        let alphabetic_count = term.chars().filter(|ch| ch.is_alphabetic()).count();
        let digit_count = term.chars().filter(|ch| ch.is_ascii_digit()).count();
        let looks_like_camel_case = raw.chars().next().is_some_and(char::is_lowercase)
            && raw.chars().skip(1).any(char::is_uppercase);
        if !(3..=48).contains(&char_count)
            || alphabetic_count < 2
            || alphabetic_count * 2 < char_count
            || digit_count > alphabetic_count
            || looks_like_camel_case
            || is_stopword(&term)
        {
            continue;
        }
        if !seen.insert(term.clone()) {
            continue;
        }
        if scores.len() >= 512 && !scores.contains_key(&term) {
            continue;
        }
        *scores.entry(term).or_default() += weight;
    }
}

fn is_stopword(term: &str) -> bool {
    matches!(
        term,
        "a" | "about"
            | "after"
            | "again"
            | "all"
            | "also"
            | "am"
            | "an"
            | "and"
            | "any"
            | "are"
            | "as"
            | "at"
            | "be"
            | "because"
            | "been"
            | "before"
            | "being"
            | "between"
            | "both"
            | "but"
            | "by"
            | "can"
            | "could"
            | "did"
            | "do"
            | "does"
            | "doing"
            | "down"
            | "during"
            | "each"
            | "few"
            | "for"
            | "from"
            | "further"
            | "had"
            | "has"
            | "have"
            | "having"
            | "he"
            | "her"
            | "here"
            | "hers"
            | "herself"
            | "him"
            | "himself"
            | "his"
            | "how"
            | "i"
            | "if"
            | "in"
            | "into"
            | "is"
            | "it"
            | "its"
            | "itself"
            | "just"
            | "may"
            | "me"
            | "more"
            | "most"
            | "my"
            | "myself"
            | "no"
            | "nor"
            | "not"
            | "now"
            | "of"
            | "off"
            | "on"
            | "once"
            | "only"
            | "or"
            | "other"
            | "our"
            | "ours"
            | "ourselves"
            | "out"
            | "over"
            | "own"
            | "same"
            | "she"
            | "should"
            | "so"
            | "some"
            | "such"
            | "than"
            | "that"
            | "the"
            | "their"
            | "theirs"
            | "them"
            | "themselves"
            | "then"
            | "there"
            | "these"
            | "they"
            | "this"
            | "those"
            | "through"
            | "to"
            | "too"
            | "under"
            | "until"
            | "up"
            | "very"
            | "was"
            | "we"
            | "were"
            | "what"
            | "when"
            | "where"
            | "which"
            | "while"
            | "who"
            | "whom"
            | "why"
            | "will"
            | "with"
            | "would"
            | "you"
            | "your"
            | "yours"
            | "yourself"
            | "yourselves"
            | "af"
            | "aldrig"
            | "alle"
            | "alt"
            | "anden"
            | "andre"
            | "blev"
            | "blive"
            | "bliver"
            | "da"
            | "de"
            | "dem"
            | "den"
            | "denne"
            | "der"
            | "det"
            | "dette"
            | "dig"
            | "din"
            | "dine"
            | "disse"
            | "du"
            | "efter"
            | "eller"
            | "en"
            | "end"
            | "er"
            | "et"
            | "fordi"
            | "fra"
            | "før"
            | "ham"
            | "han"
            | "hans"
            | "har"
            | "havde"
            | "hendes"
            | "hvis"
            | "hvordan"
            | "hvor"
            | "ikke"
            | "ind"
            | "jeg"
            | "kan"
            | "kom"
            | "kommer"
            | "kun"
            | "lidt"
            | "man"
            | "med"
            | "meget"
            | "men"
            | "mere"
            | "mig"
            | "mod"
            | "må"
            | "ned"
            | "noget"
            | "nogle"
            | "nu"
            | "og"
            | "også"
            | "om"
            | "op"
            | "os"
            | "på"
            | "samme"
            | "selv"
            | "sin"
            | "sine"
            | "skal"
            | "skulle"
            | "som"
            | "så"
            | "til"
            | "ud"
            | "var"
            | "ved"
            | "vi"
            | "vil"
            | "være"
            | "assistant"
            | "user"
            | "system"
            | "developer"
            | "tool"
            | "message"
            | "messages"
            | "conversation"
            | "conversations"
            | "title"
            | "role"
            | "author"
            | "prompt"
            | "content"
            | "chat"
            | "chats"
            | "chatgpt"
            | "codex"
            | "openai"
            | "project"
            | "projects"
            | "api"
            | "http"
            | "https"
            | "json"
            | "uuid"
            | "identifier"
            | "please"
            | "thanks"
            | "thank"
            | "hello"
            | "hi"
            | "hej"
            | "tak"
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{GraphNode, Observation, ProjectMembership};
    use serde_json::{json, Value};

    fn graph_node(id: &str, parent: Option<&str>, role: &str, content: &str) -> GraphNode {
        GraphNode {
            source_node_id: id.into(),
            source_message_id: Some(format!("message-{id}")),
            parent_id: parent.map(str::to_owned),
            role: Some(role.into()),
            author: None,
            create_time: None,
            parts: vec![Value::String(content.into())],
        }
    }

    fn ingest(
        store: &mut Store,
        provider: &str,
        scope: &str,
        source_id: &str,
        title: &str,
        nodes: Vec<GraphNode>,
        completeness: &str,
    ) -> String {
        store
            .ingest_observation(&Observation {
                provider: provider.into(),
                account_scope: scope.into(),
                source_id: source_id.into(),
                title: title.into(),
                url: None,
                current_node: nodes.last().map(|node| node.source_node_id.clone()),
                graph: nodes,
                raw_payload: json!({"update_time": 1_700_000_000.0}),
                source_time: Some("2023-11-14T22:13:20Z".into()),
                observed_ms: 1_700_000_000_000,
                parser: "fixture-parser".into(),
                completeness: completeness.into(),
            })
            .unwrap();
        store
            .connection
            .query_row(
                "SELECT id FROM conversations WHERE provider=?1 AND account_scope=?2 AND source_id=?3",
                params![provider, scope, source_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn seeded_store() -> Store {
        let mut store = Store::in_memory().unwrap();
        ingest(
            &mut store,
            "chatgpt",
            "chatgpt:observed:account-a",
            "chat-a",
            "Falcon migration plan",
            vec![
                graph_node("root", None, "user", "Falcon migration outline"),
                graph_node("left", Some("root"), "assistant", "Falcon storage plan"),
                graph_node("right", Some("root"), "assistant", "Falcon index plan"),
            ],
            "complete",
        );
        ingest(
            &mut store,
            "chatgpt",
            "chatgpt:observed:account-a",
            "chat-b",
            "Falcon deployment notes",
            vec![graph_node(
                "only",
                None,
                "user",
                "Falcon migration deployment notes",
            )],
            "complete",
        );
        ingest(
            &mut store,
            "chatgpt",
            "chatgpt:observed:account-b",
            "chat-c",
            "Falcon other account",
            vec![graph_node("other", None, "user", "Falcon private context")],
            "complete",
        );
        ingest(
            &mut store,
            "paseo-chatgpt",
            "paseo-cache:abc123",
            "archive-chat",
            "Falcon cached transcript",
            vec![graph_node(
                "archive",
                None,
                "archive",
                "User: Falcon transcript\nAssistant: answer",
            )],
            "cached_transcript",
        );
        assert!(store
            .apply_project_memberships(
                "chatgpt",
                "chatgpt:observed:account-a",
                &[ProjectMembership {
                    source_id: "chat-a".into(),
                    project_id: Some("project-explicit".into()),
                    title: Some("Falcon project".into()),
                }],
            )
            .is_ok());
        store
            .connection
            .execute(
                "UPDATE conversations SET title='Falcon project' WHERE source_id='chat-b'",
                [],
            )
            .unwrap();
        store
    }

    #[test]
    fn entity_index_classifies_links_and_keeps_file_artifacts_chat_scoped() {
        let mut store = Store::in_memory().unwrap();
        let sandbox = "sandbox:/mnt/data/report.xlsx";
        let first_id = ingest(
            &mut store,
            "chatgpt",
            "account-a",
            "entity-chat-a",
            "First source",
            vec![graph_node(
                "assistant-node",
                None,
                "assistant",
                &format!(
                    "See https://github.com/acme/chat/pull/7?token=private#diff and {sandbox}"
                ),
            )],
            "complete",
        );
        let second_id = ingest(
            &mut store,
            "chatgpt",
            "account-a",
            "entity-chat-b",
            "Second source",
            vec![graph_node("user-node", None, "user", sandbox)],
            "complete",
        );
        store.rebuild_catalog_index().unwrap();

        let page = store
            .catalog_entities("chatgpt", None, None, None, None, None, 100)
            .unwrap();
        assert!(!page.coverage_truncated);
        let prs = page
            .items
            .iter()
            .filter(|e| e.kind == CatalogEntityKind::PullRequest)
            .collect::<Vec<_>>();
        assert_eq!(prs.len(), 1);
        assert!(prs[0]
            .canonical_url
            .as_deref()
            .unwrap()
            .ends_with("/pull/7"));
        assert!(!prs[0].canonical_url.as_deref().unwrap().contains("token"));
        assert!(prs[0].repository_id.is_some());
        assert_eq!(
            page.items
                .iter()
                .filter(|e| e.kind == CatalogEntityKind::Repository)
                .count(),
            1
        );
        assert_eq!(
            page.items
                .iter()
                .filter(|e| e.kind == CatalogEntityKind::Domain)
                .count(),
            1
        );
        let files = page
            .items
            .iter()
            .filter(|e| e.kind == CatalogEntityKind::FileArtifact)
            .collect::<Vec<_>>();
        assert_eq!(
            files.len(),
            2,
            "identical sandbox paths in different chats are distinct artifacts"
        );
        assert!(files
            .iter()
            .all(|f| f.artifact_state.as_deref() == Some("link_only") && !f.content_saved));
        assert_ne!(files[0].id, files[1].id);
        let detail = store.catalog_entity(&prs[0].id).unwrap().unwrap();
        assert_eq!(
            detail.references.len(),
            2,
            "the same explicit URL is present in normalized content and parts"
        );
        assert!(detail
            .references
            .iter()
            .all(|reference| reference.conversation_id == first_id));
        assert!(detail
            .references
            .iter()
            .all(|reference| reference.source_node_id.as_deref() == Some("assistant-node")));
        assert!(detail
            .references
            .iter()
            .all(|reference| reference.role.as_deref() == Some("assistant")));
        let filtered = store
            .catalog_entities(
                "chatgpt",
                None,
                Some(&second_id),
                Some(CatalogEntityKind::FileArtifact),
                None,
                None,
                100,
            )
            .unwrap();
        assert_eq!(filtered.items.len(), 1);
        assert_eq!(filtered.items[0].filename.as_deref(), Some("report.xlsx"));
    }

    #[test]
    fn file_relation_counts_distinguish_produced_attached_and_mentioned_references() {
        let mut store = Store::in_memory().unwrap();
        let artifact_node = |node_id: &str, role: &str, part: Value| GraphNode {
            source_node_id: node_id.into(),
            source_message_id: Some(format!("message-{node_id}")),
            parent_id: None,
            role: Some(role.into()),
            author: None,
            create_time: None,
            parts: vec![part],
        };
        ingest(
            &mut store,
            "chatgpt",
            "account-a",
            "file-relation-chat",
            "File relation evidence",
            vec![
                artifact_node(
                    "generated",
                    "assistant",
                    json!({"filename":"produced.csv","file_id":"generated-file","mime_type":"text/csv"}),
                ),
                artifact_node(
                    "attached",
                    "user",
                    json!({"filename":"attached.csv","file_id":"attached-file","mime_type":"text/csv"}),
                ),
                artifact_node(
                    "mentioned",
                    "assistant",
                    json!("sandbox:/mnt/data/mentioned.csv"),
                ),
            ],
            "complete",
        );
        store.rebuild_catalog_index().unwrap();
        let page = store
            .catalog_entities(
                "chatgpt",
                None,
                None,
                Some(CatalogEntityKind::FileArtifact),
                None,
                None,
                20,
            )
            .unwrap();
        let by_filename = |filename: &str| {
            page.items
                .iter()
                .find(|entity| entity.filename.as_deref() == Some(filename))
                .unwrap_or_else(|| panic!("missing {filename}"))
        };
        let produced = by_filename("produced.csv");
        let produced_detail = store.catalog_entity(&produced.id).unwrap().unwrap();
        let produced_refs = produced_detail
            .references
            .iter()
            .filter(|reference| reference.relation == "produces")
            .count() as i64;
        assert!(produced_refs > 0);
        assert_eq!(produced.produced_reference_count, produced_refs);
        assert_eq!(produced.attachment_reference_count, 0);
        assert_eq!(produced.mentioned_reference_count, 0);
        let attached = by_filename("attached.csv");
        assert_eq!(attached.produced_reference_count, 0);
        let attached_detail = store.catalog_entity(&attached.id).unwrap().unwrap();
        let attached_refs = attached_detail
            .references
            .iter()
            .filter(|reference| reference.relation == "attaches")
            .count() as i64;
        assert!(attached_refs > 0);
        assert_eq!(attached.attachment_reference_count, attached_refs);
        assert_eq!(attached.mentioned_reference_count, 0);
        let mentioned = by_filename("mentioned.csv");
        assert_eq!(mentioned.produced_reference_count, 0);
        assert_eq!(mentioned.attachment_reference_count, 0);
        let mentioned_detail = store.catalog_entity(&mentioned.id).unwrap().unwrap();
        let mentioned_refs = mentioned_detail
            .references
            .iter()
            .filter(|reference| reference.relation == "mentions")
            .count() as i64;
        assert!(mentioned_refs > 0);
        assert_eq!(mentioned.mentioned_reference_count, mentioned_refs);

        assert_eq!(
            produced_detail.entity.produced_reference_count,
            produced_refs
        );
    }

    #[test]
    fn entity_search_filters_before_pagination_with_literal_text_source_and_tag() {
        let mut store = Store::in_memory().unwrap();
        let sources = [
            (
                "chatgpt",
                "chatgpt-scope",
                "search-chat-a",
                "special%_invoice.csv",
            ),
            (
                "chatgpt",
                "chatgpt-scope",
                "search-chat-b",
                "special-summary.csv",
            ),
            ("chatgpt", "chatgpt-scope", "search-chat-c", "ordinary.csv"),
            ("codex", "codex-scope", "search-codex", "special-codex.csv"),
        ];
        for (provider, scope, source_id, filename) in sources {
            ingest(
                &mut store,
                provider,
                scope,
                source_id,
                "Entity search fixture",
                vec![graph_node(
                    "file-link",
                    None,
                    "assistant",
                    &format!("sandbox:/mnt/data/{filename}"),
                )],
                "complete",
            );
        }
        store.rebuild_catalog_index().unwrap();
        let initial = store
            .catalog_entities_search(CatalogEntityQuery {
                source: "all",
                kind: Some(CatalogEntityKind::FileArtifact),
                query: Some("special"),
                limit: 20,
                ..CatalogEntityQuery::default()
            })
            .unwrap();
        assert_eq!(initial.total, 3);
        for entity in &initial.items {
            store
                .set_target_tags("file", &entity.id, &["search-ready".into()])
                .unwrap();
        }

        let first = store
            .catalog_entities_search(CatalogEntityQuery {
                source: "chatgpt",
                kind: Some(CatalogEntityKind::FileArtifact),
                tag: Some("search-ready"),
                query: Some("special"),
                limit: 1,
                ..CatalogEntityQuery::default()
            })
            .unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.items.len(), 1);
        let cursor = first.next_cursor.clone().expect("second matching row");
        let second = store
            .catalog_entities_search(CatalogEntityQuery {
                source: "chatgpt",
                kind: Some(CatalogEntityKind::FileArtifact),
                tag: Some("search-ready"),
                query: Some("special"),
                cursor: Some(&cursor),
                limit: 1,
                ..CatalogEntityQuery::default()
            })
            .unwrap();
        assert_eq!(second.total, 2);
        assert_eq!(second.items.len(), 1);
        assert_ne!(first.items[0].id, second.items[0].id);
        assert!(first.items[0].id < second.items[0].id);

        let literal = store
            .catalog_entities_search(CatalogEntityQuery {
                source: "chatgpt",
                kind: Some(CatalogEntityKind::FileArtifact),
                tag: Some("search-ready"),
                query: Some("%_"),
                limit: 10,
                ..CatalogEntityQuery::default()
            })
            .unwrap();
        assert_eq!(literal.total, 1, "% and _ are literal search characters");
        assert_eq!(
            literal.items[0].filename.as_deref(),
            Some("special%_invoice.csv")
        );
    }

    #[test]
    fn explicit_file_metadata_is_not_claimed_saved_and_large_fields_report_truncation() {
        let mut store = Store::in_memory().unwrap();
        let metadata_graph = GraphNode {
            source_node_id: "assistant-file".into(),
            source_message_id: Some("m-file".into()),
            parent_id: None,
            role: Some("assistant".into()),
            author: None,
            create_time: None,
            parts: vec![
                json!({"filename":"analysis.csv","file_id":"provider-file-22","mime_type":"text/csv"}),
            ],
        };
        ingest(
            &mut store,
            "chatgpt",
            "account-a",
            "metadata-chat",
            "Generated file",
            vec![metadata_graph],
            "complete",
        );
        let long_text = format!(
            "{} https://late.example.test/only-at-end",
            "x".repeat(MAX_ENTITY_SCAN_PER_FIELD + 20)
        );
        ingest(
            &mut store,
            "chatgpt",
            "account-a",
            "large-chat",
            "Large source",
            vec![graph_node("large", None, "user", &long_text)],
            "complete",
        );
        store.rebuild_catalog_index().unwrap();
        let page = store
            .catalog_entities(
                "chatgpt",
                None,
                None,
                Some(CatalogEntityKind::FileArtifact),
                None,
                None,
                100,
            )
            .unwrap();
        let artifact = page
            .items
            .iter()
            .find(|e| e.filename.as_deref() == Some("analysis.csv"))
            .unwrap();
        assert_eq!(artifact.artifact_state.as_deref(), Some("metadata_present"));
        assert!(!artifact.content_saved);
        let artifact_id = artifact.id.clone();
        let conversation_id: String = store
            .connection
            .query_row(
                "SELECT conversation_id FROM catalog_entity_refs WHERE entity_id=?1",
                [&artifact_id],
                |row| row.get(0),
            )
            .unwrap();
        store.connection.execute("INSERT INTO artifact_contents(entity_id,conversation_id,filename,mime_type,byte_size,sha256,content,saved_at_ms) VALUES(?1,?2,'analysis.csv','text/csv',3,?3,x'616263',1)",params![artifact_id,conversation_id,"a".repeat(64)]).unwrap();
        let saved_page = store
            .catalog_entities(
                "chatgpt",
                None,
                None,
                Some(CatalogEntityKind::FileArtifact),
                Some("saved_locally"),
                None,
                100,
            )
            .unwrap();
        assert_eq!(saved_page.items.len(), 1);
        assert!(saved_page.items[0].content_saved);
        assert_eq!(
            saved_page.items[0].artifact_state.as_deref(),
            Some("saved_locally")
        );
        let detail = store.catalog_entity(&artifact.id).unwrap().unwrap();
        assert!(detail.entity.content_saved);
        assert_eq!(
            detail.entity.artifact_state.as_deref(),
            Some("saved_locally")
        );
        assert_eq!(detail.references[0].relation, "produces");
        assert_eq!(
            detail.references[0].source_message_id.as_deref(),
            Some("m-file")
        );
        assert!(
            store
                .catalog_entities("chatgpt", None, None, None, None, None, 100)
                .unwrap()
                .coverage_truncated
        );
        assert!(!store
            .catalog_entities(
                "chatgpt",
                None,
                None,
                Some(CatalogEntityKind::Link),
                None,
                None,
                100
            )
            .unwrap()
            .items
            .iter()
            .any(|e| e.canonical_url.as_deref() == Some("https://late.example.test/only-at-end")));
    }

    #[test]
    fn overview_and_graph_are_account_scoped_and_project_links_are_explicit() {
        let mut store = seeded_store();
        let revision = store.library_revision().unwrap();
        let status = store.rebuild_catalog_index().unwrap();
        assert!(!status.stale);
        assert_eq!(status.library_revision, revision);
        assert_eq!(
            store.library_revision().unwrap(),
            revision,
            "derived rebuild must not bump source revision"
        );

        let overview = store.catalog_overview("chatgpt", None).unwrap();
        assert_eq!(overview.stats.chats, 4);
        assert_eq!(overview.stats.projects, 1);
        assert_eq!(overview.stats.graph_nodes, 6);
        assert_eq!(overview.stats.messages, 5);
        assert_eq!(overview.stats.branch_points, 1);
        assert_eq!(overview.stats.shared_keywords, 2);
        assert_eq!(overview.stats.provenance.chats_with_parser, 4);
        assert_eq!(overview.stats.provenance.full_graphs, 3);

        let graph = store.catalog_graph("chatgpt", None, 100).unwrap();
        assert!(
            graph.truncated,
            "the bounded map reports omitted isolated chats"
        );
        let chat_ids = graph
            .nodes
            .iter()
            .filter(|node| node.kind == "chat")
            .map(|node| node.conversation_id.as_deref().unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(chat_ids.len(), 2, "isolated chats do not become map cards");
        assert_eq!(
            graph.nodes.len(),
            2,
            "the global map contains connected chat nodes only"
        );
        assert_eq!(graph.unmapped_count, 2);
        let project_edges = graph
            .edges
            .iter()
            .filter(|edge| edge.basis == "project")
            .collect::<Vec<_>>();
        assert!(
            project_edges.is_empty(),
            "a matching title does not create a project link"
        );
        let keyword_edges = graph
            .edges
            .iter()
            .filter(|edge| edge.basis == "shared_keyword")
            .collect::<Vec<_>>();
        assert_eq!(
            keyword_edges.len(),
            1,
            "shared terms connect chats directly"
        );
        assert!(keyword_edges[0].evidence.len() >= 2);
        assert!(graph
            .nodes
            .iter()
            .all(|node| !matches!(node.label.as_str(), "assistant" | "user" | "prompt")));

        let project_key: String = store
            .connection
            .query_row(
                "SELECT project_key FROM projects WHERE source_id='project-explicit'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let filtered = store
            .catalog_graph("chatgpt", Some(&project_key), 100)
            .unwrap();
        let filtered_chats = filtered
            .nodes
            .iter()
            .filter(|node| node.kind == "chat")
            .count();
        assert_eq!(
            filtered_chats, 0,
            "single isolated project member is omitted"
        );
        assert_eq!(filtered.unmapped_count, 1);
        assert!(filtered.nodes.iter().all(|node| node.kind == "chat"));

        let chat_a_id: String = store
            .connection
            .query_row(
                "SELECT id FROM conversations WHERE source_id='chat-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let neighborhood = store
            .catalog_neighborhood(&chat_a_id, "chatgpt", None, 10)
            .unwrap();
        assert_eq!(
            neighborhood.nodes.len(),
            2,
            "two meaningful shared terms connect chats"
        );
        assert_eq!(
            neighborhood.focus_id.as_deref(),
            Some(neighborhood.nodes[0].id.as_str())
        );
        assert!(neighborhood.nodes.iter().any(|node| {
            node.source_id.as_deref() == Some("chat-b")
                && node.account_scope.as_deref() == Some("chatgpt:observed:account-a")
        }));
        assert!(neighborhood
            .edges
            .iter()
            .all(|edge| edge.basis == "shared_keyword" && edge.evidence.len() >= 2));
    }

    #[test]
    fn tagged_graphs_include_old_tagged_chats_and_exclude_untagged_neighbors() {
        let mut store = Store::in_memory().unwrap();
        let mut memberships = Vec::new();
        let mut tagged_ids = Vec::new();
        for index in 0..105 {
            let source_id = format!("recent-untagged-{index:03}");
            ingest(
                &mut store,
                "chatgpt",
                "scope-a",
                &source_id,
                &format!("Recent unrelated {index}"),
                vec![graph_node(
                    "node",
                    None,
                    "user",
                    &format!("unique topic token{index}"),
                )],
                "complete",
            );
            store
                .connection
                .execute(
                    "UPDATE conversations SET updated_at_ms=?1 WHERE source_id=?2",
                    params![1_800_000_000_000_i64 + index, source_id],
                )
                .unwrap();
        }
        for (index, source_id) in ["focus-old", "tagged-neighbor", "tagged-neighbor-two"]
            .iter()
            .enumerate()
        {
            let id = ingest(
                &mut store,
                "chatgpt",
                "scope-a",
                source_id,
                "Shared project context",
                vec![graph_node(
                    "node",
                    None,
                    "user",
                    "quartz lantern observatory",
                )],
                "complete",
            );
            store
                .connection
                .execute(
                    "UPDATE conversations SET updated_at_ms=?1 WHERE source_id=?2",
                    params![1_600_000_000_000_i64 + index as i64, source_id],
                )
                .unwrap();
            store
                .set_target_tags("conversation", &id, &["Reference Set".into()])
                .unwrap();
            tagged_ids.push((id, source_id.to_string()));
            memberships.push(ProjectMembership {
                source_id: source_id.to_string(),
                project_id: Some("shared-project".into()),
                title: Some("Shared project".into()),
            });
        }
        ingest(
            &mut store,
            "chatgpt",
            "scope-a",
            "untagged-project-peer",
            "Recent project peer",
            vec![graph_node("node", None, "user", "different content")],
            "complete",
        );
        memberships.push(ProjectMembership {
            source_id: "untagged-project-peer".into(),
            project_id: Some("shared-project".into()),
            title: Some("Shared project".into()),
        });
        store
            .apply_project_memberships("chatgpt", "scope-a", &memberships)
            .unwrap();
        store.rebuild_catalog_index().unwrap();

        let graph = store
            .catalog_graph_tagged("chatgpt", None, Some(" reference  set "), 100)
            .unwrap();
        let sources = graph
            .nodes
            .iter()
            .filter_map(|node| node.source_id.as_deref())
            .collect::<HashSet<_>>();
        assert_eq!(sources.len(), 3);
        assert!(
            sources.contains("focus-old"),
            "old tagged chats are considered before the global card cap"
        );
        assert!(!sources.contains("untagged-project-peer"));
        assert!(graph.edges.iter().all(|edge| edge.basis == "project"));

        let neighborhood = store
            .catalog_neighborhood_tagged(
                &tagged_ids[0].0,
                "chatgpt",
                None,
                Some("reference set"),
                10,
            )
            .unwrap();
        let neighbors = neighborhood
            .nodes
            .iter()
            .filter_map(|node| node.source_id.as_deref())
            .collect::<HashSet<_>>();
        assert_eq!(neighbors.len(), 3);
        assert!(!neighbors.contains("untagged-project-peer"));
        let unmatched = store
            .catalog_neighborhood_tagged(
                &tagged_ids[0].0,
                "chatgpt",
                None,
                Some("different tag"),
                10,
            )
            .unwrap();
        assert!(
            unmatched.nodes.is_empty(),
            "tag filter also restricts focus chat eligibility"
        );
    }

    #[test]
    fn cached_transcripts_are_one_archive_node_not_fake_messages_or_branches() {
        let mut store = seeded_store();
        store.rebuild_catalog_index().unwrap();
        let graph = store.catalog_graph("chatgpt", None, 100).unwrap();
        assert!(graph
            .nodes
            .iter()
            .all(|node| node.completeness.as_deref() != Some("cached_transcript")));
        assert_eq!(
            graph.unmapped_count, 2,
            "isolated archive/account chats stay out of the map"
        );
        let archived: (i64, i64, i64, String, i64) = store
            .connection
            .query_row(
                "SELECT graph_node_count,message_count,branch_count,completeness,sample_bytes FROM catalog_chats WHERE source_id='archive-chat'",
                [],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
            )
            .unwrap();
        assert_eq!(archived.0, 1);
        assert_eq!(archived.1, 0);
        assert_eq!(archived.2, 0);
        assert_eq!(archived.3, "cached_transcript");
        assert!(archived.4 <= MAX_SAMPLE_BYTES as i64);
        assert_eq!(
            store.catalog_overview("chatgpt", None).unwrap().stats.chats,
            4
        );
        assert_eq!(
            store.catalog_overview("codex", None).unwrap().stats.chats,
            0
        );
        assert_eq!(store.catalog_overview("all", None).unwrap().stats.chats, 4);
    }

    #[test]
    fn project_neighborhood_uses_exact_memberships_and_surfaces_relationship_reason() {
        let mut store = Store::in_memory().unwrap();
        ingest(
            &mut store,
            "chatgpt",
            "scope-a",
            "project-chat-a",
            "First chat",
            vec![graph_node("a", None, "user", "Orchid notebook")],
            "complete",
        );
        ingest(
            &mut store,
            "chatgpt",
            "scope-a",
            "project-chat-b",
            "Second chat",
            vec![graph_node("b", None, "user", "Marble sketchbook")],
            "complete",
        );
        ingest(
            &mut store,
            "chatgpt",
            "scope-a",
            "same-title-only",
            "Design project",
            vec![graph_node("c", None, "user", "Unrelated notes")],
            "complete",
        );
        store
            .apply_project_memberships(
                "chatgpt",
                "scope-a",
                &[
                    ProjectMembership {
                        source_id: "project-chat-a".into(),
                        project_id: Some("project-42".into()),
                        title: Some("Design project".into()),
                    },
                    ProjectMembership {
                        source_id: "project-chat-b".into(),
                        project_id: Some("project-42".into()),
                        title: Some("Design project".into()),
                    },
                ],
            )
            .unwrap();
        store.rebuild_catalog_index().unwrap();

        let focus_id: String = store
            .connection
            .query_row(
                "SELECT id FROM conversations WHERE source_id='project-chat-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let neighborhood = store
            .catalog_neighborhood(&focus_id, "chatgpt", None, 12)
            .unwrap();
        assert_eq!(neighborhood.nodes.len(), 2);
        assert!(neighborhood
            .nodes
            .iter()
            .all(|node| node.source_id.as_deref() != Some("same-title-only")));
        assert_eq!(neighborhood.edges.len(), 1);
        let edge = &neighborhood.edges[0];
        assert_eq!(edge.basis, "project");
        assert_eq!(edge.reason.as_deref(), Some("Same project: Design project"));
        assert_eq!(edge.evidence, vec!["Design project"]);
        assert_eq!(edge.score, Some(1_000_000));
    }

    #[test]
    fn keyword_candidates_need_two_terms_and_are_account_scoped() {
        let mut store = Store::in_memory().unwrap();
        ingest(
            &mut store,
            "chatgpt",
            "scope-a",
            "focus",
            "Cedar lantern field",
            vec![graph_node("a", None, "user", "Cedar lantern field")],
            "complete",
        );
        ingest(
            &mut store,
            "chatgpt",
            "scope-a",
            "one-keyword-only",
            "Cedar separate notes",
            vec![graph_node("b", None, "user", "Cedar separate notes")],
            "complete",
        );
        ingest(
            &mut store,
            "chatgpt",
            "scope-a",
            "two-keywords",
            "Lantern field guide",
            vec![graph_node("c", None, "user", "Lantern field guide")],
            "complete",
        );
        ingest(
            &mut store,
            "chatgpt",
            "scope-b",
            "other-account",
            "Cedar lantern field",
            vec![graph_node("d", None, "user", "Cedar lantern field")],
            "complete",
        );
        store.rebuild_catalog_index().unwrap();
        let focus_id: String = store
            .connection
            .query_row(
                "SELECT id FROM conversations WHERE source_id='focus'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let neighborhood = store
            .catalog_neighborhood(&focus_id, "chatgpt", None, 12)
            .unwrap();
        assert_eq!(neighborhood.nodes.len(), 2);
        assert!(neighborhood
            .nodes
            .iter()
            .any(|node| node.source_id.as_deref() == Some("two-keywords")));
        assert!(neighborhood.nodes.iter().all(|node| {
            node.source_id.as_deref() != Some("one-keyword-only")
                && node.source_id.as_deref() != Some("other-account")
        }));
        let edge = &neighborhood.edges[0];
        assert_eq!(edge.basis, "shared_keyword");
        assert_eq!(edge.evidence.len(), 2);
        assert!(edge.score.unwrap() > 0);
    }

    #[test]
    fn default_overview_caps_project_neighborhood_degree() {
        let mut store = Store::in_memory().unwrap();
        let mut memberships = Vec::new();
        for index in 0..8 {
            let source_id = format!("project-chat-{index}");
            ingest(
                &mut store,
                "chatgpt",
                "scope-a",
                &source_id,
                &format!("Project discussion {index}"),
                vec![graph_node("n", None, "user", "Different unique topic")],
                "complete",
            );
            memberships.push(ProjectMembership {
                source_id,
                project_id: Some("large-project".into()),
                title: Some("Shared project".into()),
            });
        }
        store
            .apply_project_memberships("chatgpt", "scope-a", &memberships)
            .unwrap();
        store.rebuild_catalog_index().unwrap();
        let graph = store.catalog_graph("chatgpt", None, 100).unwrap();
        assert_eq!(graph.nodes.len(), 8);
        assert!(graph.edges.len() <= MAX_EDGES_IN_GRAPH);
        for node in &graph.nodes {
            let degree = graph
                .edges
                .iter()
                .filter(|edge| edge.source == node.id || edge.target == node.id)
                .count();
            assert!(degree <= MAX_NEIGHBOR_DEGREE);
        }
        assert!(
            graph.truncated,
            "some same-project links are omitted at the degree cap"
        );
    }

    #[test]
    fn global_map_finds_older_connected_clusters_with_bounded_degree() {
        let mut store = Store::in_memory().unwrap();
        let clusters = [
            ("quartz", "migration"),
            ("cedar", "lantern"),
            ("orbital", "meadow"),
            ("harbor", "prism"),
        ];
        for (cluster_index, (first, second)) in clusters.iter().enumerate() {
            for item in 0..25 {
                let source_id = format!("cluster-{cluster_index}-{item:02}");
                ingest(
                    &mut store,
                    "chatgpt",
                    "scope-a",
                    &source_id,
                    &format!("{first} {second} case{item}"),
                    vec![graph_node(
                        "n",
                        None,
                        "user",
                        &format!("{first} {second} notes case{item}"),
                    )],
                    "complete",
                );
                store
                    .connection
                    .execute(
                        "UPDATE conversations SET updated_at_ms=?1 WHERE source_id=?2",
                        params![
                            1_700_000_000_000_i64 + cluster_index as i64 * 1_000 + item,
                            source_id
                        ],
                    )
                    .unwrap();
            }
        }
        store.rebuild_catalog_index().unwrap();

        let graph = store.catalog_graph("chatgpt", None, 100).unwrap();
        assert_eq!(graph.stats.chats, 100);
        assert_eq!(graph.nodes.len(), 100);
        assert_eq!(graph.unmapped_count, 0);
        let source_ids = graph
            .nodes
            .iter()
            .filter_map(|node| node.source_id.as_deref())
            .collect::<HashSet<_>>();
        assert!(
            source_ids.contains("cluster-0-00"),
            "old clusters are indexed beyond the latest 40 chats"
        );
        assert!(source_ids.contains("cluster-3-24"));
        assert!(graph.edges.len() > 80);
        assert!(graph.edges.iter().all(|edge| {
            edge.basis == "shared_keyword" && edge.evidence.len() >= 2 && edge.reason.is_some()
        }));
        for node in &graph.nodes {
            let degree = graph
                .edges
                .iter()
                .filter(|edge| edge.source == node.id || edge.target == node.id)
                .count();
            assert!((1..=MAX_NEIGHBOR_DEGREE).contains(&degree));
        }
    }

    #[test]
    fn stale_reads_do_not_rebuild_and_rebuild_persists_across_open() {
        let path = std::env::temp_dir().join(format!(
            "chat-hub-catalog-{}-{}.sqlite",
            std::process::id(),
            now_ms()
        ));
        let path_text = path.to_string_lossy().into_owned();
        {
            let mut store = Store::open(&path_text).unwrap();
            ingest(
                &mut store,
                "chatgpt",
                "chatgpt:observed:account-a",
                "persistent-chat",
                "Persistent index sample",
                vec![graph_node("n", None, "user", "migration topic")],
                "complete",
            );
            let before = store.library_revision().unwrap();
            store.rebuild_catalog_index().unwrap();
            assert_eq!(store.library_revision().unwrap(), before);
        }
        {
            let mut store = Store::open(&path_text).unwrap();
            let persisted = store.catalog_overview("chatgpt", None).unwrap();
            assert!(!persisted.index.stale);
            assert_eq!(persisted.stats.chats, 1);
            ingest(
                &mut store,
                "chatgpt",
                "chatgpt:observed:account-a",
                "new-chat",
                "New content appears",
                vec![graph_node("n", None, "user", "another topic")],
                "complete",
            );
            let stale = store.catalog_overview("chatgpt", None).unwrap();
            assert!(stale.index.stale);
            assert_eq!(
                stale.stats.chats, 1,
                "reads must not rebuild or mutate the derived index"
            );
            assert_eq!(
                store.library_revision().unwrap(),
                stale.index.library_revision
            );
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{path_text}-wal"));
        let _ = std::fs::remove_file(format!("{path_text}-shm"));
    }

    #[test]
    fn sample_and_graph_bounds_are_enforced() {
        let mut store = Store::in_memory().unwrap();
        let huge = "astronomyterm ".repeat(100_000);
        for index in 0..105 {
            ingest(
                &mut store,
                "chatgpt",
                "account-a",
                &format!("chat-{index:03}"),
                "Astronomy overview",
                vec![graph_node(
                    "n",
                    None,
                    "user",
                    if index == 0 { &huge } else { "astronomy topic" },
                )],
                "complete",
            );
        }
        store.rebuild_catalog_index().unwrap();
        let graph = store.catalog_graph("chatgpt", None, 200).unwrap();
        assert_eq!(
            graph
                .nodes
                .iter()
                .filter(|node| node.kind == "chat")
                .count(),
            MAX_CHATS_IN_GRAPH
        );
        assert_eq!(graph.nodes.len(), MAX_CHATS_IN_GRAPH);
        assert!(graph.nodes.iter().all(|node| node.kind == "chat"));
        assert!(graph.edges.len() <= MAX_EDGES_IN_GRAPH);
        assert!(graph.truncated);
        assert!(graph
            .nodes
            .iter()
            .filter(|node| node.kind == "chat")
            .all(|node| node.sample_bytes.unwrap() <= MAX_SAMPLE_BYTES as i64));
    }
}
