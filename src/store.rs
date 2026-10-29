use crate::error::Result;
use rusqlite::{params, Connection};

pub struct Store {
    pub(crate) connection: Connection,
}
impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let connection = Connection::open(path)?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }
    pub fn in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }
    fn migrate(&mut self) -> Result<()> {
        self.connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); CREATE TABLE IF NOT EXISTS conversations(id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '', provider TEXT NOT NULL DEFAULT '', url TEXT, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); CREATE VIRTUAL TABLE IF NOT EXISTS conversations_fts USING fts5(id UNINDEXED,title,content=''); INSERT OR IGNORE INTO schema_migrations(version) VALUES (1); CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE, parent_id TEXT, role TEXT NOT NULL, content TEXT NOT NULL DEFAULT ''); INSERT OR IGNORE INTO schema_migrations(version) VALUES (2);")?;
        Ok(())
    }
    pub fn health(&self) -> Result<bool> {
        Ok(self
            .connection
            .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?
            == 1)
    }
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self.connection.prepare(
            "SELECT id FROM conversations WHERE title LIKE ?1 ORDER BY updated_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![format!("%{query}%"), limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<String>, _>>()?)
    }
    pub fn read(&self, id: &str) -> Result<Option<serde_json::Value>> {
        let mut stmt = self
            .connection
            .prepare("SELECT id,title,provider,url,updated_at FROM conversations WHERE id=?1")?;
        let mut rows = stmt.query([id])?;
        if let Some(row) = rows.next()? {
            let mut value = serde_json::json!({"id":row.get::<_,String>(0)?,"title":row.get::<_,String>(1)?,"provider":row.get::<_,String>(2)?,"url":row.get::<_,Option<String>>(3)?,"updated_at":row.get::<_,String>(4)?});
            let mut messages = self.connection.prepare("SELECT id,parent_id,role,content FROM messages WHERE conversation_id=?1 ORDER BY rowid")?;
            let result = messages.query_map([id], |r| Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"parent_id":r.get::<_,Option<String>>(1)?,"role":r.get::<_,String>(2)?,"content":r.get::<_,String>(3)?})))?.collect::<std::result::Result<Vec<_>, _>>()?;
            value["messages"] = serde_json::json!(result);
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }
}
