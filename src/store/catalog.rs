use crate::catalog::{
    CatalogEdge, CatalogGraph, CatalogIndexStatus, CatalogNode, CatalogOverview,
    CatalogProvenanceCoverage, CatalogSourceStats, CatalogStats, CATALOG_INDEX_VERSION,
};
use crate::error::{Error, Result};
use crate::store::Store;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use std::collections::{BTreeMap, HashSet};

const MAX_SAMPLE_BYTES: usize = 64 * 1024;
const SAMPLE_CHUNK_BYTES: usize = 4 * 1024;
const MAX_SAMPLE_ROWS: usize = MAX_SAMPLE_BYTES / SAMPLE_CHUNK_BYTES;
const REBUILD_BATCH_CHATS: usize = 64;
const MAX_TERMS_PER_CHAT: usize = 10;
const MAX_CHATS_IN_GRAPH: usize = 12;
const MAX_NEIGHBOR_DEGREE: usize = 5;
const KEYWORD_CANDIDATE_SCAN: usize = 100;
const MAX_EDGES_IN_GRAPH: usize = MAX_CHATS_IN_GRAPH * MAX_NEIGHBOR_DEGREE / 2;

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

    pub fn catalog_graph(
        &self,
        source: &str,
        project_key: Option<&str>,
        limit: usize,
    ) -> Result<CatalogGraph> {
        let source_filter = source_filter(source)?;
        validate_project_filter(project_key)?;
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
        let chats = read_chat_rows(&tx, source_filter, project_key, limit)?;
        let nodes = chats.iter().map(chat_node).collect::<Vec<_>>();
        let mut edges = Vec::new();
        let mut truncated = stats.chats > chats.len() as i64;
        let mut degrees = vec![0_usize; chats.len()];
        for left in 0..chats.len() {
            for right in (left + 1)..chats.len() {
                let first = &chats[left];
                let second = &chats[right];
                if same_account(first, second)
                    && first.project_key.is_some()
                    && first.project_key == second.project_key
                {
                    if degrees[left] < MAX_NEIGHBOR_DEGREE && degrees[right] < MAX_NEIGHBOR_DEGREE {
                        edges.push(project_relationship(first, second));
                        degrees[left] += 1;
                        degrees[right] += 1;
                    } else {
                        truncated = true;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(CatalogGraph {
            index,
            stats,
            truncated,
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
        let source_filter = source_filter(source)?;
        validate_project_filter(project_key)?;
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
        let Some(focus) =
            read_chat_by_conversation(&tx, source_filter, project_key, conversation_id)?
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
            let projects =
                read_project_neighbors(&tx, source_filter, project_key, &focus, limit + 1)?;
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
        tx.commit()?;
        Ok(CatalogGraph {
            index,
            stats,
            truncated,
            focus_id: Some(focus.catalog_id),
            nodes,
            edges,
        })
    }
}

fn read_chat_rows(
    conn: &Connection,
    source_filter: &str,
    project_key: Option<&str>,
    limit: usize,
) -> Result<Vec<IndexedChat>> {
    let sql = format!(
        "SELECT catalog_id,conversation_id,provider,account_scope,source_id,title,updated_at_ms,project_key,project_label,project_label_source,graph_node_count,message_count,branch_count,source_time,observed_ms,parser,completeness,sample_bytes
         FROM catalog_chats c WHERE ({source_filter}) AND (?1 IS NULL OR c.project_key=?1)
         ORDER BY c.updated_at_ms DESC NULLS LAST,catalog_id LIMIT ?2"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params![project_key, limit as i64], indexed_chat_from_row)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn read_chat_by_conversation(
    conn: &Connection,
    source_filter: &str,
    project_key: Option<&str>,
    conversation_id: &str,
) -> Result<Option<IndexedChat>> {
    let sql = format!(
        "SELECT catalog_id,conversation_id,provider,account_scope,source_id,title,updated_at_ms,project_key,project_label,project_label_source,graph_node_count,message_count,branch_count,source_time,observed_ms,parser,completeness,sample_bytes
         FROM catalog_chats c WHERE c.conversation_id=?1 AND ({source_filter}) AND (?2 IS NULL OR c.project_key=?2) LIMIT 1"
    );
    Ok(conn
        .query_row(
            &sql,
            params![conversation_id, project_key],
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
         ORDER BY c.updated_at_ms DESC NULLS LAST,c.catalog_id LIMIT ?6"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        params![
            project_key,
            focus.catalog_id,
            focus.provider,
            focus.account_scope,
            project_filter,
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
    focus: &IndexedChat,
    limit: usize,
) -> Result<Vec<KeywordCandidate>> {
    let scope_chat_count: i64 = conn.query_row(
        "SELECT count(*) FROM catalog_chats WHERE provider=?1 AND account_scope=?2",
        params![focus.provider, focus.account_scope],
        |row| row.get(0),
    )?;
    let sql = format!(
        "SELECT c.catalog_id,c.updated_at_ms,group_concat(t.term || ':' || t.chat_count,char(31))
         FROM catalog_edges f
         JOIN catalog_terms t ON t.term_id=f.target_id
         JOIN catalog_edges e ON e.target_id=t.term_id AND e.basis='shared_keyword'
         JOIN catalog_chats c ON c.catalog_id=e.source_id
         WHERE f.source_id=?1 AND f.basis='shared_keyword' AND c.catalog_id<>?2
           AND c.provider=?3 AND c.account_scope=?4 AND ({source_filter})
           AND (?5 IS NULL OR c.project_key=?5)
         GROUP BY c.catalog_id
         HAVING count(DISTINCT t.term_id)>=2
         ORDER BY sum(1000000.0 / t.chat_count) DESC,c.updated_at_ms DESC NULLS LAST,c.catalog_id
         LIMIT ?6"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        params![
            focus.catalog_id,
            focus.catalog_id,
            focus.provider,
            focus.account_scope,
            project_filter,
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

fn same_account(left: &IndexedChat, right: &IndexedChat) -> bool {
    left.provider == right.provider && left.account_scope == right.account_scope
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

fn read_stats(
    conn: &Connection,
    source_filter: &str,
    project_key: Option<&str>,
) -> Result<CatalogStats> {
    let sql = format!(
        "SELECT count(*),coalesce(sum(c.graph_node_count),0),coalesce(sum(c.message_count),0),coalesce(sum(c.branch_count),0),max(c.updated_at_ms),
                coalesce(sum(c.source_time IS NOT NULL AND trim(c.source_time)<>''),0),
                coalesce(sum(c.parser IS NOT NULL AND trim(c.parser)<>''),0),
                coalesce(sum(c.completeness IS NOT NULL AND trim(c.completeness)<>''),0),
                coalesce(sum(c.completeness='cached_transcript'),0),
                coalesce(sum(c.completeness<>'cached_transcript' AND c.graph_node_count>0),0),
                count(DISTINCT c.project_key)
         FROM catalog_chats c WHERE ({source_filter}) AND (?1 IS NULL OR c.project_key=?1)"
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
    ): (
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
    ) = conn.query_row(&sql, params![project_key], |row| {
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
         WHERE e.basis='shared_keyword' AND ({source_filter}) AND (?1 IS NULL OR c.project_key=?1)"
    );
    let shared_keywords: i64 =
        conn.query_row(&shared_keyword_sql, params![project_key], |row| row.get(0))?;
    let source_sql = format!(
        "SELECT c.provider,count(*),coalesce(sum(c.message_count),0),coalesce(sum(c.graph_node_count),0),coalesce(sum(c.branch_count),0)
         FROM catalog_chats c WHERE ({source_filter}) AND (?1 IS NULL OR c.project_key=?1)
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
    Ok(CatalogStats {
        chats,
        projects,
        shared_keywords,
        messages,
        graph_nodes,
        branch_points,
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
        if char_count < 3
            || char_count > 48
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
        assert!(!graph.truncated);
        let chat_ids = graph
            .nodes
            .iter()
            .filter(|node| node.kind == "chat")
            .map(|node| node.conversation_id.as_deref().unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(chat_ids.len(), 4);
        assert_eq!(
            graph.nodes.len(),
            4,
            "the default overview has chat nodes only"
        );
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
        assert!(keyword_edges.is_empty(), "the overview omits keyword hubs");
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
        assert_eq!(filtered_chats, 1);
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
    fn cached_transcripts_are_one_archive_node_not_fake_messages_or_branches() {
        let mut store = seeded_store();
        store.rebuild_catalog_index().unwrap();
        let graph = store.catalog_graph("chatgpt", None, 100).unwrap();
        let archived = graph
            .nodes
            .iter()
            .find(|node| node.completeness.as_deref() == Some("cached_transcript"))
            .unwrap();
        assert_eq!(archived.graph_node_count, Some(1));
        assert_eq!(archived.message_count, Some(0));
        assert_eq!(archived.branch_count, Some(0));
        assert_eq!(archived.cached_archive, Some(true));
        assert!(archived.sample_bytes.unwrap() <= MAX_SAMPLE_BYTES as i64);
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
