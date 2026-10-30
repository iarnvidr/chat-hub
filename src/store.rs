//! SQLite-backed normalized conversation graph store.
use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

pub struct Store {
    connection: Connection,
}
#[derive(Clone, Debug)]
pub struct LibrarySnapshot {
    pub library_revision: i64,
    pub conversations: Vec<Value>,
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
#[derive(Clone, Debug)]
pub struct GraphNode {
    pub source_node_id: String,
    pub source_message_id: Option<String>,
    pub parent_id: Option<String>,
    pub role: Option<String>,
    pub author: Option<Value>,
    pub create_time: Option<Value>,
    pub parts: Vec<Value>,
}
impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let c = Connection::open(path)?;
        let mut s = Self { connection: c };
        s.migrate()?;
        Ok(s)
    }
    pub fn in_memory() -> Result<Self> {
        let c = Connection::open_in_memory()?;
        let mut s = Self { connection: c };
        s.migrate()?;
        Ok(s)
    }
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }
    fn migrate(&mut self) -> Result<()> {
        self.connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 1 {
            let tx = self.connection.transaction()?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS conversations(id TEXT PRIMARY KEY,title TEXT NOT NULL DEFAULT '',provider TEXT NOT NULL DEFAULT '',url TEXT,updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,parent_id TEXT,role TEXT NOT NULL,content TEXT NOT NULL DEFAULT '');")?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES(1)", [])?;
            tx.commit()?;
        }
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 2 {
            let tx = self.connection.transaction()?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,parent_id TEXT,role TEXT NOT NULL,content TEXT NOT NULL DEFAULT ''); CREATE VIRTUAL TABLE IF NOT EXISTS conversations_fts USING fts5(id UNINDEXED,title,content='');")?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES(2)", [])?;
            tx.commit()?;
        }
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 3 {
            let tx = self.connection.transaction()?;
            migrate_v3(&tx)?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES(3)", [])?;
            tx.commit()?;
        }
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 4 {
            let tx = self.connection.transaction()?;
            if v < 3 {
                migrate_v3(&tx)?;
                tx.execute(
                    "INSERT OR IGNORE INTO schema_migrations(version) VALUES(3)",
                    [],
                )?;
            }
            migrate_v4(&tx)?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES(4)", [])?;
            tx.commit()?;
        }
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 5 {
            let tx = self.connection.transaction()?;
            migrate_v5(&tx)?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES(5)", [])?;
            tx.commit()?;
        }
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 6 {
            let tx = self.connection.transaction()?;
            tx.execute_batch(
                "ALTER TABLE conversations ADD COLUMN last_source_updated_ms INTEGER;",
            )?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES(6)", [])?;
            tx.commit()?;
        }
        Ok(())
    }
    pub fn health(&self) -> Result<bool> {
        Ok(self
            .connection
            .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?
            == 1)
    }
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .connection
            .query_row("SELECT max(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })?)
    }
    pub fn library_revision(&self) -> Result<i64> {
        Ok(self.connection.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn library_snapshot(&self) -> Result<LibrarySnapshot> {
        let tx = self.connection.unchecked_transaction()?;
        let revision = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let mut st = tx.prepare(
            "SELECT id FROM conversations ORDER BY updated_at_ms DESC NULLS LAST,id ASC",
        )?;
        let ids = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(st);
        let mut conversations = Vec::new();
        for id in ids {
            if let Some(v) = read_normalized_conn(&tx, &id)? {
                conversations.push(v);
            }
        }
        tx.commit()?;
        Ok(LibrarySnapshot {
            library_revision: revision,
            conversations,
        })
    }
    pub fn search_snapshot(&self, query: &str) -> Result<SearchSnapshot> {
        let terms: Vec<String> = query
            .split_whitespace()
            .filter(|t| t.chars().any(char::is_alphanumeric))
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect();
        let tx = self.connection.unchecked_transaction()?;
        let revision = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let mut hits = Vec::new();
        if !terms.is_empty() {
            let q = terms.join(" AND ");
            let mut st=tx.prepare("SELECT c.id,c.provider,c.account_scope,c.source_id,c.title,c.updated_at_ms, f.node_id,f.message_id,snippet(message_fts,3,'[',']','…',12),bm25(message_fts) FROM message_fts f JOIN conversations c ON c.id=f.conversation_id WHERE message_fts MATCH ?1 ORDER BY bm25(message_fts),c.id,f.node_id")?;
            let rows = st.query_map([&q], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, f64>(9)?,
                ))
            })?;
            for row in rows {
                let (id, p, a, s, t, u, n, m, snip, score) = row?;
                if !score.is_finite() {
                    continue;
                }
                let Some(graph) = read_normalized_conn(&tx, &id)? else {
                    continue;
                };
                let nodes = graph["nodes"].as_array().cloned().unwrap_or_default();
                let selected: Vec<String> = graph["thread_node_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                let selected_branch = selected.iter().any(|selected_id| selected_id == &n);
                // Route from a match to a real descendant leaf. Visited tracking makes
                // malformed cyclic graphs finite; an unroutable cyclic hit is omitted.
                let mut frontier = vec![n.clone()];
                let mut visited = std::collections::HashSet::new();
                let mut leaves = Vec::new();
                while let Some(cursor) = frontier.pop() {
                    if !visited.insert(cursor.clone()) {
                        continue;
                    }
                    let children: Vec<String> = nodes
                        .iter()
                        .filter(|node| node["parent_id"].as_str() == Some(cursor.as_str()))
                        .filter_map(|node| node["id"].as_str().map(str::to_owned))
                        .collect();
                    if children.is_empty() {
                        leaves.push(cursor);
                    } else {
                        frontier.extend(children);
                    }
                }
                let Some(leaf) = leaves.into_iter().min() else {
                    continue;
                };
                hits.push(serde_json::json!({"conversation":{"id":id,"provider":p,"account_scope":a,"source_id":s,"title":t,"updated_at_ms":u},"node_id":n,"message_id":m,"leaf_node_id":leaf,"snippet":snip,"score":score,"selected_branch":selected_branch}));
            }
            let mut st=tx.prepare("SELECT c.id,c.provider,c.account_scope,c.source_id,c.title,c.updated_at_ms,bm25(title_fts) FROM title_fts f JOIN conversations c ON c.id=f.id WHERE title_fts MATCH ?1 ORDER BY bm25(title_fts),c.id")?;
            let rows = st.query_map([&q], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, f64>(6)?,
                ))
            })?;
            for (id, p, a, s, t, u, score) in rows.collect::<std::result::Result<Vec<_>, _>>()? {
                hits.push(serde_json::json!({"conversation":{"id":id,"provider":p,"account_scope":a,"source_id":s,"title":t,"updated_at_ms":u},"node_id":null,"message_id":null,"leaf_node_id":null,"snippet":t,"score":score,"selected_branch":false}));
            }
        }
        tx.commit()?;
        hits.sort_by(|a, b| {
            a["score"]
                .as_f64()
                .unwrap_or(0.)
                .total_cmp(&b["score"].as_f64().unwrap_or(0.))
                .then(
                    a["conversation"]["id"]
                        .as_str()
                        .cmp(&b["conversation"]["id"].as_str()),
                )
                .then(a["node_id"].as_str().cmp(&b["node_id"].as_str()))
        });
        Ok(SearchSnapshot {
            library_revision: revision,
            hits,
        })
    }
    pub fn search(&self, q: &str, limit: usize) -> Result<Vec<String>> {
        // Treat input as literal terms, never as FTS5 query syntax. Quoting each
        // whitespace-delimited term preserves AND-style matching without allowing
        // operators, column selectors, or embedded quotes to alter the query.
        let terms: Vec<String> = q
            .split_whitespace()
            .filter(|term| term.chars().any(char::is_alphanumeric))
            .map(|term| format!("\"{}\"", term.replace('\"', "\"\"")))
            .collect();
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let literal_query = terms.join(" AND ");
        let mut st = self.connection.prepare(
            "SELECT id FROM normalized_fts WHERE normalized_fts MATCH ?1 ORDER BY rank,id LIMIT ?2",
        )?;
        let rows = st.query_map(params![literal_query, limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub fn rebuild_search_index(&self) -> Result<()> {
        let tx = self.connection.unchecked_transaction()?;
        tx.execute("DELETE FROM normalized_fts", [])?;
        tx.execute("INSERT INTO normalized_fts(id,title,body) SELECT c.id,c.title,coalesce((SELECT group_concat(content,' ') FROM graph_nodes g WHERE g.conversation_id=c.id),'') FROM conversations c", [])?;
        tx.commit()?;
        Ok(())
    }
    pub fn read(&self, id: &str) -> Result<Option<Value>> {
        let mut st=self.connection.prepare("SELECT id,title,provider,url,updated_at,account_scope,source_id,current_node FROM conversations WHERE id=?1")?;
        let exact = st
            .query_row([id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })
            .optional()?;
        let (id, title, provider, url, updated, scope, source, current) = if let Some(exact) = exact
        {
            exact
        } else {
            let mut aliases = self.connection.prepare("SELECT id,title,provider,url,updated_at,account_scope,source_id,current_node FROM conversations WHERE source_id=?1")?;
            let mut rows = aliases.query([id])?;
            let first = rows.next()?;
            let Some(row) = first else {
                return Ok(None);
            };
            let found = (
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
            );
            if rows.next()?.is_some() {
                return Ok(None);
            }
            found
        };
        let mut ns=self.connection.prepare("SELECT source_node_id,source_message_id,parent_id,role,author_json,create_time_json,parts_json,content FROM graph_nodes WHERE conversation_id=?1 ORDER BY source_node_id")?;
        let nodes=ns.query_map([&id],|r|Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"source_message_id":r.get::<_,Option<String>>(1)?,"parent_id":r.get::<_,Option<String>>(2)?,"role":r.get::<_,Option<String>>(3)?,"author":r.get::<_,Option<String>>(4)?.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"create_time":r.get::<_,Option<String>>(5)?.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"parts":serde_json::from_str::<Value>(&r.get::<_,String>(6)?).unwrap_or(Value::Null),"content":r.get::<_,String>(7)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
        let messages = nodes
            .iter()
            .filter(|n| !n["parts"].is_null())
            .cloned()
            .collect::<Vec<_>>();
        Ok(Some(
            serde_json::json!({"id":id,"title":title,"provider":provider,"url":url,"updated_at":updated,"account_scope":scope,"source_id":source,"current_node":current,"nodes":nodes,"messages":messages}),
        ))
    }
    pub fn read_normalized(&self, id: &str) -> Result<Option<Value>> {
        read_normalized_conn(&self.connection, id)
    }

    pub fn ingest_observation(&mut self, o: &Observation) -> Result<String> {
        validate(o)?;
        let tx = self.connection.transaction()?;
        ingest_tx(&tx, o)?;
        let id = tx.query_row(
            "SELECT id FROM conversations WHERE provider=?1 AND account_scope=?2 AND source_id=?3",
            params![o.provider, o.account_scope, o.source_id],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(id)
    }
}
fn read_normalized_conn(conn: &Connection, id: &str) -> Result<Option<Value>> {
    let row = conn.query_row("SELECT id,title,provider,account_scope,source_id,url,current_node,updated_at_ms FROM conversations WHERE id=?1", [id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<i64>>(7)?))).optional()?;
    let Some((id, title, provider, scope, source, url, current, updated)) = row else {
        return Ok(None);
    };
    let mut st=conn.prepare("SELECT source_node_id,source_message_id,message_id,parent_id,role,author_json,create_time_json,parts_json,content FROM graph_nodes WHERE conversation_id=?1 ORDER BY source_node_id")?;
    let rows = st.query_map([&id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
        ))
    })?;
    let mut nodes = Vec::new();
    let mut messages = Vec::new();
    for row in rows {
        let (nid, sid, mid, parent, role, author, time, parts, content) = row?;
        let raw: Value = serde_json::from_str(&parts).unwrap_or(Value::Null);
        let p = raw
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                if let Some(t) = v.as_str() {
                    serde_json::json!({"kind":"text","text":t,"raw":v})
                } else {
                    serde_json::json!({"kind":"unknown","raw":v})
                }
            })
            .collect::<Vec<_>>();
        let millis = time
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.as_f64())
            .map(|n| (n * 1000.0).floor() as i64);
        nodes.push(
            serde_json::json!({"id":nid,"source_id":sid,"parent_id":parent,"message_id":mid}),
        );
        if let (Some(mid), Some(role)) = (mid, role) {
            messages.push(serde_json::json!({"id":mid,"source_id":sid,"node_id":nid,"role":role,"author":author.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"create_time":time.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"created_at_ms":millis,"content":content,"parts":p}));
        }
    }
    let reason = if current
        .as_ref()
        .is_some_and(|c| nodes.iter().any(|n| n["id"] == *c))
    {
        "source_current_node"
    } else {
        "lexical_leaf_fallback"
    };
    let selected = current
        .clone()
        .filter(|c| nodes.iter().any(|n| n["id"] == *c))
        .or_else(|| {
            nodes
                .iter()
                .filter(|n| !nodes.iter().any(|x| x["parent_id"] == n["id"]))
                .map(|n| n["id"].as_str().unwrap_or("").to_owned())
                .min()
        });
    let mut ancestry = Vec::new();
    let mut cursor = selected.clone();
    while let Some(c) = cursor {
        if ancestry.contains(&c) {
            break;
        }
        ancestry.push(c.clone());
        cursor = nodes
            .iter()
            .find(|n| n["id"] == c)
            .and_then(|n| n["parent_id"].as_str().map(str::to_owned));
    }
    Ok(Some(
        serde_json::json!({"id":id,"title":title,"provider":provider,"scope":scope,"source_id":source,"url":url,"updated_at_ms":updated,"source_current_node_id":current,"selected_node_id":selected,"selection_reason":reason,"thread_node_ids":ancestry,"thread_complete":ancestry.last().is_none_or(|n|nodes.iter().any(|x|x["id"]==*n&&x["parent_id"].is_null())),"nodes":nodes,"messages":messages}),
    ))
}

fn migrate_v5(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("CREATE TABLE library_state(singleton INTEGER PRIMARY KEY CHECK(singleton=1),revision INTEGER NOT NULL); INSERT INTO library_state VALUES(1,CASE WHEN EXISTS(SELECT 1 FROM conversations) THEN 1 ELSE 0 END); CREATE VIRTUAL TABLE message_fts USING fts5(conversation_id UNINDEXED,node_id UNINDEXED,message_id UNINDEXED,content); CREATE VIRTUAL TABLE title_fts USING fts5(id UNINDEXED,title); INSERT INTO message_fts SELECT g.conversation_id,g.source_node_id,g.message_id,g.content FROM graph_nodes g WHERE g.role IS NOT NULL OR g.source_message_id IS NOT NULL; INSERT INTO title_fts SELECT id,title FROM conversations; CREATE TRIGGER message_fts_insert AFTER INSERT ON graph_nodes BEGIN INSERT INTO message_fts VALUES(new.conversation_id,new.source_node_id,new.message_id,new.content); END; CREATE TRIGGER message_fts_delete AFTER DELETE ON graph_nodes BEGIN DELETE FROM message_fts WHERE conversation_id=old.conversation_id AND node_id=old.source_node_id; END; CREATE TRIGGER message_fts_update AFTER UPDATE ON graph_nodes BEGIN DELETE FROM message_fts WHERE conversation_id=old.conversation_id AND node_id=old.source_node_id; INSERT INTO message_fts SELECT new.conversation_id,new.source_node_id,new.message_id,new.content WHERE new.role IS NOT NULL OR new.source_message_id IS NOT NULL; END; CREATE TRIGGER title_fts_insert AFTER INSERT ON conversations BEGIN INSERT INTO title_fts VALUES(new.id,new.title); END; CREATE TRIGGER title_fts_update AFTER UPDATE OF title ON conversations BEGIN UPDATE title_fts SET title=new.title WHERE id=new.id; END; CREATE TRIGGER title_fts_delete AFTER DELETE ON conversations BEGIN DELETE FROM title_fts WHERE id=old.id; END;")?;
    Ok(())
}

fn migrate_v4(tx: &Transaction<'_>) -> Result<()> {
    let mut cols = tx.prepare("PRAGMA table_info(graph_nodes)")?;
    let names = cols
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(cols);
    if !names.iter().any(|n| n == "message_id") {
        tx.execute_batch("ALTER TABLE graph_nodes ADD COLUMN message_id TEXT")?;
    }
    let mut cols = tx.prepare("PRAGMA table_info(conversations)")?;
    let names = cols
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(cols);
    if !names.iter().any(|n| n == "updated_at_ms") {
        tx.execute_batch("ALTER TABLE conversations ADD COLUMN updated_at_ms INTEGER")?;
    }
    tx.execute("UPDATE graph_nodes SET message_id=source_message_id WHERE message_id IS NULL AND EXISTS(SELECT 1 FROM messages m WHERE m.id=graph_nodes.source_message_id AND m.conversation_id=graph_nodes.conversation_id)", [])?;
    Ok(())
}
fn migrate_v3(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("ALTER TABLE conversations ADD COLUMN account_scope TEXT NOT NULL DEFAULT 'provisional:default'; ALTER TABLE conversations ADD COLUMN source_id TEXT NOT NULL DEFAULT ''; ALTER TABLE conversations ADD COLUMN current_node TEXT; UPDATE conversations SET source_id=id WHERE source_id=''; CREATE UNIQUE INDEX IF NOT EXISTS conversations_identity ON conversations(provider,account_scope,source_id); CREATE TABLE IF NOT EXISTS observations(id INTEGER PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,source_time TEXT,observed_ms INTEGER NOT NULL,parser TEXT NOT NULL,completeness TEXT NOT NULL,raw_payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS graph_nodes(conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,source_node_id TEXT NOT NULL,source_message_id TEXT,message_id TEXT,parent_id TEXT,role TEXT,author_json TEXT,create_time_json TEXT,parts_json TEXT NOT NULL,content TEXT NOT NULL DEFAULT '',PRIMARY KEY(conversation_id,source_node_id)); CREATE VIRTUAL TABLE IF NOT EXISTS normalized_fts USING fts5(id UNINDEXED,title,body);")?;
    tx.execute_batch("CREATE TRIGGER IF NOT EXISTS normalized_conv_insert AFTER INSERT ON conversations BEGIN INSERT INTO normalized_fts(id,title,body) VALUES(new.id,new.title,coalesce((SELECT group_concat(content,' ') FROM graph_nodes WHERE conversation_id=new.id),'')); END; CREATE TRIGGER IF NOT EXISTS normalized_conv_update AFTER UPDATE OF title ON conversations BEGIN UPDATE normalized_fts SET title=new.title WHERE id=new.id; END; CREATE TRIGGER IF NOT EXISTS normalized_conv_delete AFTER DELETE ON conversations BEGIN DELETE FROM normalized_fts WHERE id=old.id; END; CREATE TRIGGER IF NOT EXISTS normalized_graph_insert AFTER INSERT ON graph_nodes BEGIN UPDATE normalized_fts SET body=coalesce((SELECT group_concat(content,' ') FROM graph_nodes WHERE conversation_id=new.conversation_id),'') WHERE id=new.conversation_id; END; CREATE TRIGGER IF NOT EXISTS normalized_graph_update AFTER UPDATE OF content ON graph_nodes BEGIN UPDATE normalized_fts SET body=coalesce((SELECT group_concat(content,' ') FROM graph_nodes WHERE conversation_id=new.conversation_id),'') WHERE id=new.conversation_id; END; CREATE TRIGGER IF NOT EXISTS normalized_graph_delete AFTER DELETE ON graph_nodes BEGIN UPDATE normalized_fts SET body=coalesce((SELECT group_concat(content,' ') FROM graph_nodes WHERE conversation_id=old.conversation_id),'') WHERE id=old.conversation_id; END;")?;
    {
        let mut q = tx.prepare("SELECT id,conversation_id,parent_id,role,content FROM messages")?;
        let rows = q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (id, c, parent, role, content) = row?;
            let parts = serde_json::to_string(&vec![Value::String(content.clone())]).unwrap();
            tx.execute("INSERT INTO graph_nodes(conversation_id,source_node_id,source_message_id,parent_id,role,parts_json,content) VALUES(?1,?2,?2,?3,?4,?5,?6)",params![c,id,parent,role,parts,content])?;
        }
    }
    tx.execute("INSERT INTO normalized_fts(id,title,body) SELECT c.id,c.title,coalesce((SELECT group_concat(content,' ') FROM graph_nodes g WHERE g.conversation_id=c.id),'') FROM conversations c",[])?;
    Ok(())
}
fn canonical_conversation_id(o: &Observation) -> String {
    format!(
        "conversation:{}",
        serde_json::json!([o.provider, o.account_scope, o.source_id])
    )
}

fn canonical_message_id(conversation_id: &str, node_id: &str) -> String {
    format!("message:{}", serde_json::json!([conversation_id, node_id]))
}

// IDs are globally unique in the legacy tables, so deterministic candidates must
// be checked against their owner rather than treating a textual prefix as reserved.
fn allocate_id(
    tx: &Transaction<'_>,
    table: &str,
    base: &str,
    conversation: Option<&str>,
) -> Result<String> {
    for suffix in 0u64.. {
        let candidate = if suffix == 0 {
            base.to_owned()
        } else {
            format!("{base}~{suffix}")
        };
        let owner: Option<String> = match (table, conversation) {
            ("conversations", None) => tx
                .query_row(
                    "SELECT id FROM conversations WHERE id=?1",
                    [&candidate],
                    |r| r.get(0),
                )
                .optional()?,
            ("messages", Some(_)) => tx
                .query_row(
                    "SELECT conversation_id FROM messages WHERE id=?1",
                    [&candidate],
                    |r| r.get(0),
                )
                .optional()?,
            _ => unreachable!(),
        };
        if owner.is_none() {
            return Ok(candidate);
        }
    }
    unreachable!()
}

fn validate(o: &Observation) -> Result<()> {
    if o.provider.trim().is_empty()
        || o.account_scope.trim().is_empty()
        || o.source_id.trim().is_empty()
        || o.completeness.trim().is_empty()
    {
        return Err(crate::error::Error::Configuration(
            "provider, account scope, source id and completeness are required".into(),
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for n in &o.graph {
        if n.source_node_id.is_empty() || !ids.insert(&n.source_node_id) {
            return Err(crate::error::Error::Configuration(
                "node IDs must be nonempty and unique per observation".into(),
            ));
        }
    }
    Ok(())
}
type CanonicalGraph = Vec<(
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
)>;
type PriorState = (String, Option<String>, Option<String>, Option<i64>);
fn ingest_tx(tx: &Transaction<'_>, o: &Observation) -> Result<()> {
    let id: String = if let Some(existing) = tx
        .query_row(
            "SELECT id FROM conversations WHERE provider=?1 AND account_scope=?2 AND source_id=?3",
            params![o.provider, o.account_scope, o.source_id],
            |r| r.get(0),
        )
        .optional()?
    {
        existing
    } else {
        allocate_id(tx, "conversations", &canonical_conversation_id(o), None)?
    };
    let source_ms = o
        .raw_payload
        .get("update_time")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .map(|n| (n * 1000.0).floor() as i64);
    let prior: Option<PriorState> = tx
        .query_row(
            "SELECT title,url,current_node,last_source_updated_ms FROM conversations WHERE id=?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let stale =
        matches!((source_ms,prior.as_ref().and_then(|x|x.3)),(Some(new),Some(old)) if new<=old);
    let accept = !stale;
    let before_graph: CanonicalGraph = {
        let mut st=tx.prepare("SELECT source_node_id,source_message_id,parent_id,role,author_json,create_time_json,parts_json FROM graph_nodes WHERE conversation_id=?1 ORDER BY source_node_id")?;
        let rows = st
            .query_map([&id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?
            .collect::<std::result::Result<_, _>>()?;
        rows
    };
    if accept {
        tx.execute("INSERT INTO conversations(id,title,provider,account_scope,source_id,url,current_node,last_source_updated_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(provider,account_scope,source_id) DO UPDATE SET title=excluded.title,url=coalesce(excluded.url,conversations.url),current_node=excluded.current_node,last_source_updated_ms=coalesce(excluded.last_source_updated_ms,conversations.last_source_updated_ms)",params![id,o.title,o.provider,o.account_scope,o.source_id,o.url,o.current_node,source_ms])?;
    } else {
        tx.execute("INSERT OR IGNORE INTO conversations(id,title,provider,account_scope,source_id,url,current_node) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,o.title,o.provider,o.account_scope,o.source_id,o.url,o.current_node])?;
    }
    for n in if accept { &o.graph[..] } else { &[] } {
        let parts = serde_json::to_string(&n.parts).unwrap();
        let content = n
            .parts
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .join("\n");
        tx.execute("INSERT INTO graph_nodes(conversation_id,source_node_id,source_message_id,parent_id,role,author_json,create_time_json,parts_json,content) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(conversation_id,source_node_id) DO UPDATE SET source_message_id=coalesce(excluded.source_message_id,graph_nodes.source_message_id),parent_id=excluded.parent_id,role=coalesce(excluded.role,graph_nodes.role),author_json=coalesce(excluded.author_json,graph_nodes.author_json),create_time_json=coalesce(excluded.create_time_json,graph_nodes.create_time_json),parts_json=excluded.parts_json,content=excluded.content",params![id,n.source_node_id,n.source_message_id,n.parent_id,n.role,n.author.as_ref().map(Value::to_string),n.create_time.as_ref().map(Value::to_string),parts,content])?;
        if n.role.is_none() && n.source_message_id.is_none() && n.author.is_none() {
            continue;
        }
        let mid = canonical_message_id(&id, &n.source_node_id);
        let persisted_id: Option<String> = tx
            .query_row(
                "SELECT message_id FROM graph_nodes WHERE conversation_id=?1 AND source_node_id=?2",
                params![id, n.source_node_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let message_id = match persisted_id {
            Some(v) => v,
            None => allocate_id(tx, "messages", &mid, Some(&id))?,
        };
        tx.execute(
            "UPDATE graph_nodes SET message_id=?3 WHERE conversation_id=?1 AND source_node_id=?2",
            params![id, n.source_node_id, message_id],
        )?;
        let parent_id: Option<String> = None;
        tx.execute("INSERT INTO messages(id,conversation_id,parent_id,role,content) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET parent_id=excluded.parent_id,role=excluded.role,content=excluded.content",params![message_id,id,parent_id,n.role.as_deref().unwrap_or("unknown"),content])?;
    }
    if accept {
        tx.execute("UPDATE messages SET parent_id=(SELECT parent.message_id FROM graph_nodes child JOIN graph_nodes parent ON parent.conversation_id=child.conversation_id AND parent.source_node_id=child.parent_id WHERE child.conversation_id=?1 AND child.message_id=messages.id) WHERE conversation_id=?1", [&id])?;
    }
    tx.execute("INSERT INTO observations(conversation_id,source_time,observed_ms,parser,completeness,raw_payload) VALUES(?1,?2,?3,?4,?5,?6)",params![id,o.source_time,o.observed_ms,o.parser,o.completeness,o.raw_payload.to_string()])?;
    let after_graph: CanonicalGraph = {
        let mut st=tx.prepare("SELECT source_node_id,source_message_id,parent_id,role,author_json,create_time_json,parts_json FROM graph_nodes WHERE conversation_id=?1 ORDER BY source_node_id")?;
        let rows = st
            .query_map([&id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?
            .collect::<std::result::Result<_, _>>()?;
        rows
    };
    let persisted: (String, Option<String>, Option<String>) = tx.query_row(
        "SELECT title,url,current_node FROM conversations WHERE id=?1",
        [&id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let changed = accept
        && (prior.as_ref().map(|p| (&p.0, &p.1, &p.2))
            != Some((&persisted.0, &persisted.1, &persisted.2))
            || before_graph != after_graph);
    if changed {
        tx.execute(
            "UPDATE library_state SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        tx.execute(
            "UPDATE conversations SET updated_at_ms=?2 WHERE id=?1",
            params![
                id,
                if accept {
                    source_ms.unwrap_or(o.observed_ms)
                } else {
                    o.observed_ms
                }
            ],
        )?;
    }
    if accept {
        tx.execute("UPDATE normalized_fts SET title=?2,body=coalesce((SELECT group_concat(content,' ') FROM graph_nodes WHERE conversation_id=?1),'') WHERE id=?1",params![id,persisted.0])?;
    }
    Ok(())
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn migrated_legacy_ids_and_rows_survive_ingest() {
        let mut store = Store::in_memory().unwrap();
        store.connection.execute_batch("DROP TRIGGER IF EXISTS message_fts_insert; DROP TRIGGER IF EXISTS message_fts_delete; DROP TRIGGER IF EXISTS message_fts_update; DROP TRIGGER IF EXISTS title_fts_insert; DROP TRIGGER IF EXISTS title_fts_update; DROP TRIGGER IF EXISTS title_fts_delete; DROP TABLE IF EXISTS message_fts; DROP TABLE IF EXISTS title_fts; DROP TABLE IF EXISTS library_state; DROP TABLE normalized_fts; DROP TABLE conversations_fts; DROP INDEX conversations_identity; DROP TRIGGER normalized_conv_insert; DROP TRIGGER normalized_conv_update; DROP TRIGGER normalized_conv_delete; DROP TRIGGER normalized_graph_insert; DROP TRIGGER normalized_graph_update; DROP TRIGGER normalized_graph_delete; DROP TABLE graph_nodes; DROP TABLE observations; DROP TABLE messages; DROP TABLE conversations; CREATE TABLE conversations(id TEXT PRIMARY KEY,title TEXT NOT NULL DEFAULT '',provider TEXT NOT NULL DEFAULT '',url TEXT,updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); CREATE TABLE messages(id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id),parent_id TEXT,role TEXT NOT NULL,content TEXT NOT NULL DEFAULT ''); INSERT INTO conversations(id,title,provider) VALUES('legacy-conv','old','legacy'); INSERT INTO messages(id,conversation_id,parent_id,role,content) VALUES('legacy-msg','legacy-conv',NULL,'user','kept'); DELETE FROM schema_migrations WHERE version>=3;").unwrap();
        store.migrate().unwrap();
        let observation = Observation {
            provider: "legacy".into(),
            account_scope: "provisional:default".into(),
            source_id: "legacy-conv".into(),
            title: "legacy".into(),
            url: None,
            current_node: None,
            graph: vec![GraphNode {
                source_node_id: "legacy-msg".into(),
                source_message_id: Some("different-provider-id".into()),
                parent_id: None,
                role: Some("user".into()),
                author: None,
                create_time: None,
                parts: vec![Value::String("updated".into())],
            }],
            raw_payload: Value::Null,
            source_time: None,
            observed_ms: 1,
            parser: "test".into(),
            completeness: "complete".into(),
        };
        store.ingest_observation(&observation).unwrap();
        let id: String = store
            .connection
            .query_row(
                "SELECT id FROM messages WHERE conversation_id='legacy-conv'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let content: String = store
            .connection
            .query_row(
                "SELECT content FROM messages WHERE id='legacy-msg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(id, "legacy-msg");
        assert_eq!(content, "updated");
    }
}
