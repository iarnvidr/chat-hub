//! SQLite-backed normalized conversation graph store.
use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

pub struct Store {
    pub(crate) connection: Connection,
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
    pub fn search(&self, q: &str, limit: usize) -> Result<Vec<String>> {
        let mut st = self.connection.prepare(
            "SELECT id FROM normalized_fts WHERE normalized_fts MATCH ?1 ORDER BY rank,id LIMIT ?2",
        )?;
        let rows = st.query_map(params![q, limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
    pub fn rebuild_search_index(&self) -> Result<()> {
        self.connection.execute("DELETE FROM normalized_fts", [])?;
        self.connection.execute("INSERT INTO normalized_fts(id,title,body) SELECT c.id,c.title,coalesce((SELECT group_concat(content,' ') FROM graph_nodes g WHERE g.conversation_id=c.id),'') FROM conversations c", [])?;
        Ok(())
    }
    pub fn read(&self, id: &str) -> Result<Option<Value>> {
        let mut st=self.connection.prepare("SELECT id,title,provider,url,updated_at,account_scope,source_id,current_node FROM conversations WHERE id=?1 OR source_id=?1 ORDER BY id LIMIT 1")?;
        let Some((id, title, provider, url, updated, scope, source, current)) = st
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
            .optional()?
        else {
            return Ok(None);
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
fn migrate_v3(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("ALTER TABLE conversations ADD COLUMN account_scope TEXT NOT NULL DEFAULT 'provisional:default'; ALTER TABLE conversations ADD COLUMN source_id TEXT NOT NULL DEFAULT ''; ALTER TABLE conversations ADD COLUMN current_node TEXT; UPDATE conversations SET source_id=id WHERE source_id=''; CREATE UNIQUE INDEX IF NOT EXISTS conversations_identity ON conversations(provider,account_scope,source_id); CREATE TABLE IF NOT EXISTS observations(id INTEGER PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,source_time TEXT,observed_ms INTEGER NOT NULL,parser TEXT NOT NULL,completeness TEXT NOT NULL,raw_payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS graph_nodes(conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,source_node_id TEXT NOT NULL,source_message_id TEXT,parent_id TEXT,role TEXT,author_json TEXT,create_time_json TEXT,parts_json TEXT NOT NULL,content TEXT NOT NULL DEFAULT '',PRIMARY KEY(conversation_id,source_node_id)); CREATE VIRTUAL TABLE IF NOT EXISTS normalized_fts USING fts5(id UNINDEXED,title,body);")?;
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
fn ingest_tx(tx: &Transaction<'_>, o: &Observation) -> Result<()> {
    let id: String = tx
        .query_row(
            "SELECT id FROM conversations WHERE provider=?1 AND account_scope=?2 AND source_id=?3",
            params![o.provider, o.account_scope, o.source_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_else(|| canonical_conversation_id(o));
    tx.execute("INSERT INTO conversations(id,title,provider,account_scope,source_id,url,current_node) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(provider,account_scope,source_id) DO UPDATE SET title=excluded.title,url=coalesce(excluded.url,conversations.url),current_node=CASE WHEN excluded.current_node IS NOT NULL AND EXISTS(SELECT 1 FROM graph_nodes WHERE conversation_id=conversations.id AND source_node_id=excluded.current_node) THEN excluded.current_node ELSE conversations.current_node END",params![id,o.title,o.provider,o.account_scope,o.source_id,o.url,o.current_node])?;
    for n in &o.graph {
        let parts = serde_json::to_string(&n.parts).unwrap();
        let content = n
            .parts
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .join("\n");
        tx.execute("INSERT INTO graph_nodes(conversation_id,source_node_id,source_message_id,parent_id,role,author_json,create_time_json,parts_json,content) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(conversation_id,source_node_id) DO UPDATE SET source_message_id=coalesce(excluded.source_message_id,graph_nodes.source_message_id),parent_id=excluded.parent_id,role=coalesce(excluded.role,graph_nodes.role),author_json=coalesce(excluded.author_json,graph_nodes.author_json),create_time_json=coalesce(excluded.create_time_json,graph_nodes.create_time_json),parts_json=excluded.parts_json,content=excluded.content",params![id,n.source_node_id,n.source_message_id,n.parent_id,n.role,n.author.as_ref().map(Value::to_string),n.create_time.as_ref().map(Value::to_string),parts,content])?;
        let mid = canonical_message_id(&id, &n.source_node_id);
        let existing_legacy_id: Option<String> = tx
            .query_row(
                "SELECT id FROM messages WHERE conversation_id=?1 AND id=?2",
                params![id, n.source_node_id],
                |r| r.get(0),
            )
            .optional()?;
        let message_id = existing_legacy_id.unwrap_or(mid);
        let parent_id = n
            .parent_id
            .as_deref()
            .map(|parent| canonical_message_id(&id, parent));
        tx.execute("INSERT INTO messages(id,conversation_id,parent_id,role,content) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET parent_id=excluded.parent_id,role=excluded.role,content=excluded.content",params![message_id,id,parent_id,n.role.as_deref().unwrap_or("unknown"),content])?;
    }
    tx.execute("INSERT INTO observations(conversation_id,source_time,observed_ms,parser,completeness,raw_payload) VALUES(?1,?2,?3,?4,?5,?6)",params![id,o.source_time,o.observed_ms,o.parser,o.completeness,o.raw_payload.to_string()])?;
    tx.execute("UPDATE normalized_fts SET title=?2,body=coalesce((SELECT group_concat(content,' ') FROM graph_nodes WHERE conversation_id=?1),'') WHERE id=?1",params![id,o.title])?;
    Ok(())
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn migrated_legacy_ids_and_rows_survive_ingest() {
        let mut store = Store::in_memory().unwrap();
        store.connection.execute_batch("DROP TABLE normalized_fts; DROP TABLE conversations_fts; DROP INDEX conversations_identity; DROP TRIGGER normalized_conv_insert; DROP TRIGGER normalized_conv_update; DROP TRIGGER normalized_conv_delete; DROP TRIGGER normalized_graph_insert; DROP TRIGGER normalized_graph_update; DROP TRIGGER normalized_graph_delete; DROP TABLE graph_nodes; DROP TABLE observations; DROP TABLE messages; DROP TABLE conversations; CREATE TABLE conversations(id TEXT PRIMARY KEY,title TEXT NOT NULL DEFAULT '',provider TEXT NOT NULL DEFAULT '',url TEXT,updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); CREATE TABLE messages(id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id),parent_id TEXT,role TEXT NOT NULL,content TEXT NOT NULL DEFAULT ''); INSERT INTO conversations(id,title,provider) VALUES('legacy-conv','old','legacy'); INSERT INTO messages(id,conversation_id,parent_id,role,content) VALUES('legacy-msg','legacy-conv',NULL,'user','kept'); DELETE FROM schema_migrations WHERE version=3;").unwrap();
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
