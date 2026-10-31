//! SQLite-backed normalized conversation graph store.
use crate::error::{Error, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

#[cfg(test)]
thread_local! { static CANDIDATE_MATERIALIZATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

pub struct Store {
    connection: Connection,
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
    pub updated_at_ms: Option<i64>,
    pub id: String,
}
#[derive(Clone, Debug)]
pub struct SearchKey {
    pub score: f64,
    pub conversation_id: String,
    pub node_id: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ListPageSnapshot {
    pub library_revision: i64,
    pub items: Vec<Value>,
}
#[derive(Clone, Debug)]
pub struct SearchPageSnapshot {
    pub library_revision: i64,
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
type ProjectRecord = (String, String, String, Option<String>, Option<String>);
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
impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let c = Connection::open(path)?;
        let mut s = Self { connection: c };
        s.migrate()?;
        Ok(s)
    }
    /// Open and migrate an existing regular-file database without creating it.
    pub fn open_existing(path: &str) -> Result<Self> {
        let file_path = std::path::Path::new(path);
        if path == ":memory:" || !file_path.is_file() {
            return Err(Error::Configuration(
                "database must be an existing regular file".into(),
            ));
        }
        let c = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
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
    /// Read-only connection to an already migrated service-owned database.
    /// It has its own transaction/mutex so imports do not block library reads.
    pub(crate) fn open_reader(path: &str) -> Result<Self> {
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON;")?;
        Ok(Self { connection })
    }
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }
    pub(crate) fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
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
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 7 {
            let tx = self.connection.transaction()?;
            tx.execute_batch("ALTER TABLE conversations ADD COLUMN ordering_payload TEXT;")?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES(7)", [])?;
            tx.commit()?;
        }
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 8 {
            let tx = self.connection.transaction()?;
            tx.execute_batch("ALTER TABLE conversations ADD COLUMN ordering_observed_ms INTEGER;")?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES(8)", [])?;
            tx.commit()?;
        }
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 9 {
            let tx = self.connection.transaction()?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS projects(project_key TEXT PRIMARY KEY,provider TEXT NOT NULL,account_scope TEXT NOT NULL,source_id TEXT NOT NULL,title TEXT,UNIQUE(provider,account_scope,source_id)); CREATE TABLE IF NOT EXISTS conversation_projects(conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,project_key TEXT NOT NULL REFERENCES projects(project_key) ON DELETE CASCADE); CREATE INDEX IF NOT EXISTS conversation_projects_project ON conversation_projects(project_key,conversation_id); INSERT OR IGNORE INTO projects(project_key,provider,account_scope,source_id,title) SELECT 'project:'||json_array(c.provider,c.account_scope,json_extract(o.raw_payload,'$.paseo_cache.project')),c.provider,c.account_scope,json_extract(o.raw_payload,'$.paseo_cache.project'),NULL FROM conversations c JOIN observations o ON o.conversation_id=c.id AND o.id=(SELECT max(latest.id) FROM observations latest WHERE latest.conversation_id=c.id) WHERE c.provider='paseo-chatgpt' AND json_type(o.raw_payload,'$.paseo_cache.project')='text' AND trim(json_extract(o.raw_payload,'$.paseo_cache.project'))<>''; INSERT OR IGNORE INTO conversation_projects(conversation_id,project_key) SELECT c.id,'project:'||json_array(c.provider,c.account_scope,json_extract(o.raw_payload,'$.paseo_cache.project')) FROM conversations c JOIN observations o ON o.conversation_id=c.id AND o.id=(SELECT max(latest.id) FROM observations latest WHERE latest.conversation_id=c.id) WHERE c.provider='paseo-chatgpt' AND json_type(o.raw_payload,'$.paseo_cache.project')='text' AND trim(json_extract(o.raw_payload,'$.paseo_cache.project'))<>'';")?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES(9)", [])?;
            tx.commit()?;
        }
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 10 {
            let tx = self.connection.transaction()?;
            let mappings = tx.execute("DELETE FROM conversation_projects WHERE project_key IN (SELECT project_key FROM projects WHERE trim(source_id)='')", [])?;
            let projects = tx.execute("DELETE FROM projects WHERE trim(source_id)=''", [])?;
            if mappings > 0 || projects > 0 {
                tx.execute(
                    "UPDATE library_state SET revision=revision+1 WHERE singleton=1",
                    [],
                )?;
            }
            tx.execute("INSERT INTO schema_migrations(version) VALUES(10)", [])?;
            tx.commit()?;
        }
        let v: i64 = self.connection.query_row(
            "SELECT coalesce(max(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if v < 11 {
            let tx = self.connection.transaction()?;
            tx.execute_batch("ALTER TABLE projects ADD COLUMN local_name TEXT;")?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES(11)", [])?;
            tx.commit()?;
        }
        self.connection.execute_batch("CREATE INDEX IF NOT EXISTS conversations_page_order ON conversations(updated_at_ms DESC,id ASC); CREATE INDEX IF NOT EXISTS graph_nodes_conversation_order ON graph_nodes(conversation_id,source_node_id); CREATE INDEX IF NOT EXISTS observations_conversation_order ON observations(conversation_id,id DESC);")?;
        Ok(())
    }
    pub(crate) fn migrate_for_restore(&mut self) -> Result<()> {
        self.migrate()
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
    pub fn apply_project_memberships(
        &mut self,
        provider: &str,
        account_scope: &str,
        memberships: &[ProjectMembership],
    ) -> Result<()> {
        let tx = self.connection.transaction()?;
        let mut changed = false;
        for membership in memberships {
            let conversation_id: Option<String> = tx.query_row("SELECT id FROM conversations WHERE provider=?1 AND account_scope=?2 AND source_id=?3", params![provider,account_scope,membership.source_id], |r| r.get(0)).optional()?;
            let Some(conversation_id) = conversation_id else {
                return Err(Error::Configuration(
                    "project mapping references unknown conversation".into(),
                ));
            };
            if let Some(project_id) = membership.project_id.as_deref() {
                if project_id.is_empty() || project_id.len() > 256 {
                    return Err(Error::Configuration("invalid project id".into()));
                }
                let key = project_key(provider, account_scope, project_id);
                let old_title: Option<Option<String>> = tx.query_row("SELECT title FROM projects WHERE provider=?1 AND account_scope=?2 AND source_id=?3",params![provider,account_scope,project_id],|r|r.get(0)).optional()?;
                if old_title.is_none()
                    || (membership.title.is_some()
                        && old_title.flatten().as_ref() != membership.title.as_ref())
                {
                    changed = true;
                }
                tx.execute("INSERT INTO projects(project_key,provider,account_scope,source_id,title) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(provider,account_scope,source_id) DO UPDATE SET title=coalesce(excluded.title,projects.title)",params![key,provider,account_scope,project_id,membership.title.as_deref().filter(|s| !s.trim().is_empty())])?;
                let key: String = tx.query_row("SELECT project_key FROM projects WHERE provider=?1 AND account_scope=?2 AND source_id=?3",params![provider,account_scope,project_id],|r|r.get(0))?;
                let prior: Option<String> = tx
                    .query_row(
                        "SELECT project_key FROM conversation_projects WHERE conversation_id=?1",
                        [&conversation_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if prior.as_deref() != Some(key.as_str()) {
                    changed = true;
                }
                tx.execute("INSERT INTO conversation_projects(conversation_id,project_key) VALUES(?1,?2) ON CONFLICT(conversation_id) DO UPDATE SET project_key=excluded.project_key",params![conversation_id,key])?;
            } else {
                changed |= tx.execute(
                    "DELETE FROM conversation_projects WHERE conversation_id=?1",
                    [&conversation_id],
                )? > 0;
            }
        }
        if changed {
            tx.execute(
                "UPDATE library_state SET revision=revision+1 WHERE singleton=1",
                [],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Refresh titles from the authenticated ChatGPT sidebar without requiring
    /// conversation-list or per-project conversation requests to succeed.
    /// Only projects already attributed to this exact provider/account scope
    /// are updated; the sidebar cannot create memberships or claim other rows.
    pub fn apply_project_titles(
        &mut self,
        provider: &str,
        account_scope: &str,
        projects: &[ProjectTitle],
    ) -> Result<()> {
        if provider != "chatgpt"
            || !account_scope
                .strip_prefix("chatgpt:observed:")
                .is_some_and(|digest| {
                    digest.len() == 64
                        && digest
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                })
        {
            return Err(Error::Configuration(
                "invalid observed ChatGPT scope".into(),
            ));
        }
        let tx = self.connection.transaction()?;
        let mut changed = false;
        for project in projects {
            if project.source_id.is_empty() || project.source_id.len() > 256 {
                return Err(Error::Configuration("invalid project id".into()));
            }
            let title = project
                .title
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty());
            if title.is_some_and(|name| name.len() > 4096) {
                return Err(Error::Configuration("invalid project title".into()));
            }
            let Some(title) = title else { continue };
            changed |= tx.execute(
                "UPDATE projects SET title=?1 WHERE source_id=?2 AND title IS NOT ?1 AND ((provider='chatgpt' AND account_scope=?3) OR (provider='chatgpt' AND account_scope LIKE 'provisional:%') OR (provider='chatgpt' AND account_scope LIKE 'paseo-cache:%') OR (provider='paseo-chatgpt' AND account_scope LIKE 'paseo-cache:%'))",
                params![title, project.source_id, account_scope],
            )? > 0;
        }
        if changed {
            tx.execute(
                "UPDATE library_state SET revision=revision+1 WHERE singleton=1",
                [],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn projects_snapshot(&self, source: &str) -> Result<ProjectSnapshot> {
        let tx = self.connection.unchecked_transaction()?;
        let revision: i64 = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let filter = match source {
            "chatgpt" => "p.provider IN ('chatgpt','paseo-chatgpt')",
            "codex" => "p.provider LIKE 'codex%' OR p.provider LIKE 'paseo-codex%'",
            "all" => "1=1",
            _ => return Err(Error::Configuration("invalid source".into())),
        };
        let sql = format!("SELECT p.project_key,p.source_id,p.title,p.local_name,count(cp.conversation_id) FROM projects p LEFT JOIN conversation_projects cp ON cp.project_key=p.project_key WHERE {filter} GROUP BY p.project_key,p.source_id,p.title,p.local_name ORDER BY coalesce(nullif(trim(p.local_name),''),nullif(trim(p.title),''),p.source_id) COLLATE NOCASE,p.project_key");
        let mut st = tx.prepare(&sql)?;
        let projects = st.query_map([], |r| {
            let local_name: Option<String> = r.get(3)?;
            let title: Option<String> = r.get(2)?;
            let (display_name, name_source) = project_display_name(local_name.as_deref(), title.as_deref());
            Ok(serde_json::json!({"project_key":r.get::<_,String>(0)?,"project_id":r.get::<_,String>(1)?,"title":title,"display_name":display_name,"name_source":name_source,"conversation_count":r.get::<_,i64>(4)?}))
        })?.collect::<std::result::Result<Vec<_>,_>>()?;
        drop(st);
        tx.commit()?;
        Ok(ProjectSnapshot {
            library_revision: revision,
            projects,
        })
    }
    pub fn rename_project(
        &mut self,
        project_key: &str,
        name: Option<&str>,
    ) -> Result<ProjectRenameSnapshot> {
        if project_key.is_empty() || project_key.len() > 1024 {
            return Err(Error::Configuration("invalid project key".into()));
        }
        let normalized = name.map(str::trim).filter(|value| !value.is_empty());
        if normalized
            .is_some_and(|value| value.chars().count() > 120 || value.chars().any(char::is_control))
        {
            return Err(Error::Configuration("invalid project name".into()));
        }
        let tx = self.connection.transaction()?;
        let prior: Option<(String, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT source_id,local_name,title FROM projects WHERE project_key=?1",
                [project_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((source_id, old_name, title)) = prior else {
            return Err(Error::Configuration("unknown project".into()));
        };
        let normalized = normalized.map(str::to_owned);
        let changed = old_name != normalized;
        if changed {
            tx.execute(
                "UPDATE projects SET local_name=?1 WHERE project_key=?2",
                params![normalized, project_key],
            )?;
            tx.execute(
                "UPDATE library_state SET revision=revision+1 WHERE singleton=1",
                [],
            )?;
        }
        let revision: i64 = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let (display_name, name_source) =
            project_display_name(normalized.as_deref(), title.as_deref());
        let project = serde_json::json!({"project_key":project_key,"project_id":source_id,"title":title,"display_name":display_name,"name_source":name_source});
        tx.commit()?;
        Ok(ProjectRenameSnapshot {
            library_revision: revision,
            project,
        })
    }
    pub fn project_assignment_context(
        &self,
        conversation_id: &str,
        project_key: Option<&str>,
    ) -> Result<Value> {
        let conversation: Option<(String, String, String)> = self
            .connection
            .query_row(
                "SELECT provider,account_scope,source_id FROM conversations WHERE id=?1",
                [conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((conversation_provider, conversation_scope, conversation_source_id)) =
            conversation
        else {
            return Err(Error::Configuration("unknown conversation".into()));
        };
        if !is_chatgpt_provider(&conversation_provider)
            || !valid_upstream_conversation_id(&conversation_source_id)
        {
            return Err(Error::Configuration(
                "conversation is not an upstream ChatGPT record".into(),
            ));
        }
        let project = if let Some(key) = project_key {
            if key.is_empty() || key.len() > 1024 {
                return Err(Error::Configuration("invalid project key".into()));
            }
            let value: Option<ProjectRecord> = self
                .connection
                .query_row(
                    "SELECT provider,account_scope,source_id,title,local_name FROM projects WHERE project_key=?1",
                    [key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .optional()?;
            let Some((provider, account_scope, source_id, title, local_name)) = value else {
                return Err(Error::Configuration("unknown project".into()));
            };
            if !is_chatgpt_provider(&provider) || !valid_upstream_project_id(&source_id) {
                return Err(Error::Configuration(
                    "project is not an upstream ChatGPT project".into(),
                ));
            }
            if observed_account_scope(&conversation_scope)
                && observed_account_scope(&account_scope)
                && conversation_scope != account_scope
            {
                return Err(Error::Configuration(
                    "conversation and project belong to different observed accounts".into(),
                ));
            }
            Some((
                key.to_owned(),
                provider,
                account_scope,
                source_id,
                title,
                local_name,
            ))
        } else {
            None
        };
        let (project_key, project_provider, project_scope, project_id, project_title, local_name) =
            project.map_or(
                (None, None, None, None, None, None),
                |(key, provider, scope, id, title, local)| {
                    (
                        Some(key),
                        Some(provider),
                        Some(scope),
                        Some(id),
                        title,
                        local,
                    )
                },
            );
        Ok(serde_json::json!({
            "conversation_id": conversation_id,
            "conversation_provider": conversation_provider,
            "conversation_scope": conversation_scope,
            "conversation_source_id": conversation_source_id,
            "project_key": project_key,
            "project_provider": project_provider,
            "project_scope": project_scope,
            "project_id": project_id,
            "project_title": project_title,
            "project_local_name": local_name,
        }))
    }
    pub fn commit_project_assignment(
        &mut self,
        conversation_id: &str,
        expected_conversation_scope: &str,
        project_key: Option<&str>,
        verified_project_title: Option<&str>,
    ) -> Result<Value> {
        let tx = self.connection.transaction()?;
        let conversation: Option<(String, String, String)> = tx
            .query_row(
                "SELECT provider,account_scope,source_id FROM conversations WHERE id=?1",
                [conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((provider, account_scope, source_id)) = conversation else {
            return Err(Error::Configuration("unknown conversation".into()));
        };
        if !is_chatgpt_provider(&provider)
            || account_scope != expected_conversation_scope
            || !valid_upstream_conversation_id(&source_id)
        {
            return Err(Error::Configuration(
                "conversation changed before project assignment commit".into(),
            ));
        }
        let mut changed = false;
        let result_project = if let Some(project_key) = project_key {
            let project: Option<ProjectRecord> = tx
                .query_row(
                    "SELECT provider,source_id,account_scope,title,local_name FROM projects WHERE project_key=?1",
                    [project_key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .optional()?;
            let Some((project_provider, project_id, project_scope, old_title, local_name)) =
                project
            else {
                return Err(Error::Configuration(
                    "project changed before assignment commit".into(),
                ));
            };
            if !is_chatgpt_provider(&project_provider) || !valid_upstream_project_id(&project_id) {
                return Err(Error::Configuration(
                    "project is not an upstream ChatGPT project".into(),
                ));
            }
            if observed_account_scope(&account_scope)
                && observed_account_scope(&project_scope)
                && account_scope != project_scope
            {
                return Err(Error::Configuration(
                    "conversation and project belong to different observed accounts".into(),
                ));
            }
            let title = verified_project_title
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if title.is_some_and(|value| {
                value.chars().count() > 4096 || value.chars().any(char::is_control)
            }) {
                return Err(Error::Configuration(
                    "invalid verified project title".into(),
                ));
            }
            if let Some(title) = title {
                if old_title.as_deref() != Some(title) {
                    tx.execute(
                        "UPDATE projects SET title=?1 WHERE project_key=?2",
                        params![title, project_key],
                    )?;
                    changed = true;
                }
            }
            let prior: Option<String> = tx
                .query_row(
                    "SELECT project_key FROM conversation_projects WHERE conversation_id=?1",
                    [conversation_id],
                    |row| row.get(0),
                )
                .optional()?;
            if prior.as_deref() != Some(project_key) {
                tx.execute("INSERT INTO conversation_projects(conversation_id,project_key) VALUES(?1,?2) ON CONFLICT(conversation_id) DO UPDATE SET project_key=excluded.project_key", params![conversation_id,project_key])?;
                changed = true;
            }
            let (display_name, name_source) =
                project_display_name(local_name.as_deref(), title.or(old_title.as_deref()));
            Some(
                serde_json::json!({"project_key":project_key,"project_id":project_id,"title":title.or(old_title.as_deref()),"display_name":display_name,"name_source":name_source}),
            )
        } else {
            changed |= tx.execute(
                "DELETE FROM conversation_projects WHERE conversation_id=?1",
                [conversation_id],
            )? > 0;
            None
        };
        if changed {
            tx.execute(
                "UPDATE library_state SET revision=revision+1 WHERE singleton=1",
                [],
            )?;
        }
        let revision: i64 = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        tx.commit()?;
        Ok(
            serde_json::json!({"conversation_id":conversation_id,"project":result_project,"library_revision":revision,"provider":provider,"account_scope":account_scope}),
        )
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
        let terms = literal_search_terms(query);
        let tx = self.connection.unchecked_transaction()?;
        let revision = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let mut hits = Vec::new();
        if !terms.is_empty() {
            let q = terms.join(" AND ");
            let mut st=tx.prepare("SELECT c.id,c.provider,c.account_scope,c.source_id,c.title,c.updated_at_ms, f.node_id,f.message_id,snippet(message_fts,3,'','','…',12),bm25(message_fts) FROM message_fts f JOIN conversations c ON c.id=f.conversation_id WHERE message_fts MATCH ?1 ORDER BY bm25(message_fts),c.id,f.node_id")?;
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
            let mut st=tx.prepare("SELECT c.id,c.provider,c.account_scope,c.source_id,c.title,c.updated_at_ms,bm25(title_fts)*2.0 FROM title_fts f JOIN conversations c ON c.id=f.id WHERE title_fts MATCH ?1 ORDER BY bm25(title_fts)*2.0,c.id")?;
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
    pub fn list_page_snapshot(
        &self,
        limit: usize,
        after: Option<&ListKey>,
        expected_revision: Option<i64>,
    ) -> Result<ListPageSnapshot> {
        self.list_page_snapshot_filtered(limit, after, expected_revision, false)
    }
    /// Chat-side listing keeps the existing keyset, revision, and response-budget
    /// contract while restricting candidates to ChatGPT conversation providers.
    pub fn chat_list_page_snapshot(
        &self,
        limit: usize,
        after: Option<&ListKey>,
        expected_revision: Option<i64>,
    ) -> Result<ListPageSnapshot> {
        self.list_page_snapshot_filtered(limit, after, expected_revision, true)
    }
    pub fn source_list_page_snapshot(
        &self,
        limit: usize,
        after: Option<&ListKey>,
        expected_revision: Option<i64>,
        source: &str,
        project_key: Option<&str>,
    ) -> Result<ListPageSnapshot> {
        self.list_page_snapshot_scoped(limit, after, expected_revision, source, project_key)
    }
    fn list_page_snapshot_filtered(
        &self,
        limit: usize,
        after: Option<&ListKey>,
        expected_revision: Option<i64>,
        chat_only: bool,
    ) -> Result<ListPageSnapshot> {
        self.list_page_snapshot_scoped(
            limit,
            after,
            expected_revision,
            if chat_only { "chatgpt" } else { "all" },
            None,
        )
    }
    fn list_page_snapshot_scoped(
        &self,
        limit: usize,
        after: Option<&ListKey>,
        expected_revision: Option<i64>,
        source: &str,
        project_key: Option<&str>,
    ) -> Result<ListPageSnapshot> {
        let tx = self.connection.unchecked_transaction()?;
        let revision: i64 = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let mut items = Vec::new();
        if limit == 0 {
            return Err(Error::Configuration("page limit must be at least 1".into()));
        }
        if expected_revision.is_none_or(|v| v == revision) {
            let lim = limit.min(101) as i64;
            let source_filter = match source {
                "all" => "",
                "chatgpt" => "provider IN ('chatgpt','paseo-chatgpt') AND ",
                "codex" => "(provider LIKE 'codex%' OR provider LIKE 'paseo-codex%') AND ",
                _ => return Err(Error::Configuration("invalid source".into())),
            };
            let project_filter = "(?6 IS NULL OR EXISTS(SELECT 1 FROM conversation_projects cp WHERE cp.conversation_id=conversations.id AND cp.project_key=?6)) AND ";
            let budget_sql = format!("SELECT coalesce(sum(length(CAST(id AS BLOB))+length(CAST(provider AS BLOB))+length(CAST(account_scope AS BLOB))+length(CAST(source_id AS BLOB))+length(CAST(title AS BLOB))),0) FROM (SELECT id,provider,account_scope,source_id,title FROM conversations WHERE {source_filter}{project_filter}(?1=1 OR (?2 IS NOT NULL AND (updated_at_ms < ?2 OR (updated_at_ms=?2 AND id>?3) OR updated_at_ms IS NULL)) OR (?2 IS NULL AND ?4=1 AND updated_at_ms IS NULL AND id>?3)) ORDER BY updated_at_ms DESC NULLS LAST,id ASC LIMIT ?5)");
            let mut pre = tx.prepare(&budget_sql)?;
            let source_bytes: i64 = pre.query_row(
                params![
                    after.is_none() as i64,
                    after.and_then(|k| k.updated_at_ms),
                    after.map(|k| k.id.as_str()).unwrap_or(""),
                    after.is_some() as i64,
                    lim,
                    project_key
                ],
                |r| r.get(0),
            )?;
            if (source_bytes.max(0) as usize)
                .saturating_mul(6)
                .saturating_add(lim as usize * 256)
                > 8 * 1024 * 1024 - 64 * 1024
            {
                return Err(Error::ReadBudgetExceeded {
                    kind: "list encoded bytes",
                    limit: 8 * 1024 * 1024 - 64 * 1024,
                });
            }
            let page_sql = format!("SELECT id,provider,account_scope,source_id,title,updated_at_ms,(SELECT json_object('project_key',p.project_key,'project_id',p.source_id,'title',p.title,'display_name',CASE WHEN trim(coalesce(p.local_name,''))<>'' THEN trim(p.local_name) WHEN trim(coalesce(p.title,''))<>'' THEN trim(p.title) ELSE NULL END,'name_source',CASE WHEN trim(coalesce(p.local_name,''))<>'' THEN 'local' WHEN trim(coalesce(p.title,''))<>'' THEN 'upstream' ELSE 'unknown' END) FROM conversation_projects cp JOIN projects p ON p.project_key=cp.project_key WHERE cp.conversation_id=conversations.id) FROM conversations WHERE {source_filter}{project_filter}(?1=1 OR (?2 IS NOT NULL AND (updated_at_ms < ?2 OR (updated_at_ms=?2 AND id>?3) OR updated_at_ms IS NULL)) OR (?2 IS NULL AND ?4=1 AND updated_at_ms IS NULL AND id>?3)) ORDER BY updated_at_ms DESC NULLS LAST,id ASC LIMIT ?5");
            let mut st = tx.prepare(&page_sql)?;
            let ids = st
                .query_map(
                    params![after.is_none() as i64, after.and_then(|k| k.updated_at_ms), after.map(|k| k.id.as_str()).unwrap_or(""), after.is_some() as i64, lim, project_key],
                    |r| { let project: Option<String> = r.get(6)?; Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"provider":r.get::<_,String>(1)?,"account_scope":r.get::<_,String>(2)?,"source_id":r.get::<_,String>(3)?,"title":r.get::<_,String>(4)?,"updated_at_ms":r.get::<_,Option<i64>>(5)?,"project":project.and_then(|value|serde_json::from_str::<Value>(&value).ok())})) },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            items.extend(ids);
        }
        tx.commit()?;
        Ok(ListPageSnapshot {
            library_revision: revision,
            items,
        })
    }
    pub fn detail_snapshot(&self, canonical_id: &str) -> Result<DetailSnapshot> {
        let tx = self.connection.unchecked_transaction()?;
        let revision = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM graph_nodes WHERE conversation_id=?1",
            [canonical_id],
            |r| r.get(0),
        )?;
        if count > 100_000 {
            return Err(Error::ReadBudgetExceeded {
                kind: "detail nodes",
                limit: 100_000,
            });
        }
        // Bound raw material before asking SQLite to inspect JSON or constructing a
        // DTO. Then budget the actual JSON string escaping and normalized part
        // duplication. Multiplying all source bytes by six incorrectly rejects
        // ordinary cached transcripts that fit comfortably in the response limit.
        const RESPONSE_LIMIT: usize = 8 * 1024 * 1024 - 64 * 1024;
        let bytes: i64 = tx.query_row("SELECT coalesce(sum(length(CAST(id AS BLOB))+length(CAST(provider AS BLOB))+length(CAST(account_scope AS BLOB))+length(CAST(source_id AS BLOB))+length(CAST(title AS BLOB))+coalesce(length(CAST(url AS BLOB)),0)+coalesce(length(CAST(current_node AS BLOB)),0)),0) FROM conversations WHERE id=?1", [canonical_id], |r| r.get(0))?;
        let bytes = bytes.saturating_add(tx.query_row("SELECT coalesce(sum(length(CAST(source_node_id AS BLOB))+coalesce(length(CAST(source_message_id AS BLOB)),0)+coalesce(length(CAST(message_id AS BLOB)),0)+coalesce(length(CAST(parent_id AS BLOB)),0)+coalesce(length(CAST(role AS BLOB)),0)+coalesce(length(CAST(author_json AS BLOB)),0)+coalesce(length(CAST(create_time_json AS BLOB)),0)+length(CAST(parts_json AS BLOB))+length(CAST(content AS BLOB))),0) FROM graph_nodes WHERE conversation_id=?1", [canonical_id], |r| r.get::<_,i64>(0))?);
        if bytes.max(0) as usize > RESPONSE_LIMIT {
            return Err(Error::ReadBudgetExceeded {
                kind: "detail source bytes",
                limit: RESPONSE_LIMIT,
            });
        }
        // json_quote counts UTF-8 bytes after escaping, without transferring the
        // text to Rust. A node ID occurs in the node twice, its message and the
        // selected ancestry. Message IDs occur in the node and message. Text
        // parts occur twice (text/raw); unknown parts occur once. Doubling the
        // original parts JSON and allowing 48 bytes per part covers both forms.
        // Fixed wrapper/number/null fields fit within 512 bytes per node and
        // another 512 bytes for the conversation. The selected fallback node
        // gets a separate allowance because it need not equal current_node.
        let encoded: i64 = tx.query_row(
            "SELECT coalesce(sum(length(CAST(json_quote(id) AS BLOB))+length(CAST(json_quote(provider) AS BLOB))+2*length(CAST(json_quote(account_scope) AS BLOB))+length(CAST(json_quote(source_id) AS BLOB))+length(CAST(json_quote(title) AS BLOB))+length(CAST(json_quote(url) AS BLOB))+length(CAST(json_quote(current_node) AS BLOB))),0) FROM conversations WHERE id=?1",
            [canonical_id], |r| r.get(0),
        )?;
        let encoded = encoded.saturating_add(tx.query_row(
            "SELECT coalesce(sum(4*length(CAST(json_quote(source_node_id) AS BLOB))+length(CAST(json_quote(source_message_id) AS BLOB))+2*length(CAST(json_quote(message_id) AS BLOB))+length(CAST(json_quote(parent_id) AS BLOB))+length(CAST(json_quote(role) AS BLOB))+coalesce(length(CAST(author_json AS BLOB)),4)+coalesce(length(CAST(create_time_json AS BLOB)),4)+2*length(CAST(parts_json AS BLOB))+48*json_array_length(parts_json)+length(CAST(json_quote(content) AS BLOB))+512),0)+coalesce(max(length(CAST(json_quote(source_node_id) AS BLOB))),0) FROM graph_nodes WHERE conversation_id=?1",
            [canonical_id], |r| r.get::<_, i64>(0),
        )?).saturating_add(512);
        if encoded.max(0) as usize > RESPONSE_LIMIT {
            return Err(Error::ReadBudgetExceeded {
                kind: "detail encoded bytes",
                limit: RESPONSE_LIMIT,
            });
        }
        let conversation = read_normalized_conn(&tx, canonical_id)?;
        if let Some(value) = &conversation {
            let encoded =
                serde_json::to_vec(value).map_err(|e| Error::Configuration(e.to_string()))?;
            if encoded.len() > RESPONSE_LIMIT {
                return Err(Error::ReadBudgetExceeded {
                    kind: "detail encoded bytes",
                    limit: RESPONSE_LIMIT,
                });
            }
        }
        tx.commit()?;
        Ok(DetailSnapshot {
            library_revision: revision,
            conversation,
        })
    }
    pub fn search_page_snapshot(
        &self,
        raw_query: &str,
        limit: usize,
        after: Option<&SearchKey>,
        expected_revision: Option<i64>,
    ) -> Result<SearchPageSnapshot> {
        self.search_page_snapshot_filtered(raw_query, limit, after, expected_revision, false)
    }
    /// Chat-side search uses the same FTS ranking and bounded keyset walk as the
    /// all-source search, with the provider predicate applied inside FTS results.
    pub fn chat_search_page_snapshot(
        &self,
        raw_query: &str,
        limit: usize,
        after: Option<&SearchKey>,
        expected_revision: Option<i64>,
    ) -> Result<SearchPageSnapshot> {
        self.search_page_snapshot_filtered(raw_query, limit, after, expected_revision, true)
    }
    pub fn source_search_page_snapshot(
        &self,
        raw_query: &str,
        limit: usize,
        after: Option<&SearchKey>,
        expected_revision: Option<i64>,
        source: &str,
        project_key: Option<&str>,
    ) -> Result<SearchPageSnapshot> {
        self.search_page_snapshot_scoped(
            raw_query,
            limit,
            after,
            expected_revision,
            source,
            project_key,
        )
    }
    fn search_page_snapshot_filtered(
        &self,
        raw_query: &str,
        limit: usize,
        after: Option<&SearchKey>,
        expected_revision: Option<i64>,
        chat_only: bool,
    ) -> Result<SearchPageSnapshot> {
        self.search_page_snapshot_scoped(
            raw_query,
            limit,
            after,
            expected_revision,
            if chat_only { "chatgpt" } else { "all" },
            None,
        )
    }
    fn search_page_snapshot_scoped(
        &self,
        raw_query: &str,
        limit: usize,
        after: Option<&SearchKey>,
        expected_revision: Option<i64>,
        source: &str,
        project_key: Option<&str>,
    ) -> Result<SearchPageSnapshot> {
        let terms = literal_search_terms(raw_query);
        if after.is_some_and(|k| !k.score.is_finite()) {
            return Err(crate::error::Error::Configuration(
                "search cursor score must be finite".into(),
            ));
        }
        let tx = self.connection.unchecked_transaction()?;
        let revision: i64 = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let mut hits = Vec::new();
        if limit == 0 {
            return Err(Error::Configuration("page limit must be at least 1".into()));
        }
        if expected_revision.is_none_or(|v| v == revision) && !terms.is_empty() {
            let q = terms.join(" AND ");
            let mut cursor_score = after.map(|k| k.score);
            let mut cursor_conversation =
                after.map(|k| k.conversation_id.clone()).unwrap_or_default();
            let mut cursor_node = after.and_then(|k| k.node_id.clone());
            let mut scanned = 0usize;
            let mut cache = std::collections::HashMap::<String, SearchTopology>::new();
            let mut total = 0usize;
            let mut total_bytes = 0usize;
            while hits.len() < limit.min(101) && scanned < 1024 {
                let lim = (limit.min(101) - hits.len()).min(1024 - scanned) as i64;
                let source_message_filter = match source { "all" => "", "chatgpt" => " AND f.conversation_id IN (SELECT id FROM conversations WHERE provider IN ('chatgpt','paseo-chatgpt'))", "codex" => " AND f.conversation_id IN (SELECT id FROM conversations WHERE provider LIKE 'codex%' OR provider LIKE 'paseo-codex%')", _ => return Err(Error::Configuration("invalid source".into())) };
                let source_title_filter = match source {
                    "all" => "",
                    "chatgpt" => " AND c.provider IN ('chatgpt','paseo-chatgpt')",
                    "codex" => " AND (c.provider LIKE 'codex%' OR c.provider LIKE 'paseo-codex%')",
                    _ => return Err(Error::Configuration("invalid source".into())),
                };
                let project_message_filter = " AND (?6 IS NULL OR f.conversation_id IN (SELECT conversation_id FROM conversation_projects WHERE project_key=?6))";
                let project_title_filter = " AND (?6 IS NULL OR EXISTS(SELECT 1 FROM conversation_projects cp WHERE cp.conversation_id=c.id AND cp.project_key=?6))";
                let chat_message_filter =
                    format!("{source_message_filter}{project_message_filter}");
                let chat_title_filter = format!("{source_title_filter}{project_title_filter}");
                // FTS counts tokens, not bytes: a single token may contain an
                // entire long URL or pasted blob. Bound the excerpt separately
                // while keeping the complete text in the authoritative store.
                let sql = format!("WITH results AS (SELECT f.conversation_id,f.node_id,f.message_id,substr(snippet(message_fts,3,'','','…',12),1,2048) snippet,bm25(message_fts) score FROM message_fts f WHERE message_fts MATCH ?1{chat_message_filter} UNION ALL SELECT f.id,NULL,NULL,c.title,bm25(title_fts)*2.0 FROM title_fts f JOIN conversations c ON c.id=f.id WHERE title_fts MATCH ?1{chat_title_filter}) SELECT conversation_id,node_id,message_id,snippet,score, (length(CAST(conversation_id AS BLOB))+coalesce(length(CAST(node_id AS BLOB)),0)+coalesce(length(CAST(message_id AS BLOB)),0)+length(CAST(snippet AS BLOB))+coalesce((SELECT length(CAST(title AS BLOB)) FROM conversations WHERE id=results.conversation_id),0))*6+512 FROM results WHERE (?2 IS NULL OR score>?2 OR (score=?2 AND (conversation_id>?3 OR (conversation_id=?3 AND (node_id>?4 OR (?4 IS NULL AND node_id IS NOT NULL)))))) ORDER BY score,conversation_id,node_id LIMIT ?5");
                let mut st = tx.prepare(&sql)?;
                let mut prospective_bytes = total_bytes;
                let rows = st
                    .query_map(
                        params![
                            q,
                            cursor_score,
                            cursor_conversation,
                            cursor_node,
                            lim,
                            project_key
                        ],
                        |r| {
                            let estimate = r.get::<_, i64>(5)?;
                            let candidate_bytes = estimate.max(0) as usize;
                            let budget: usize = 8 * 1024 * 1024 - 64 * 1024;
                            if candidate_bytes > budget.saturating_sub(prospective_bytes) {
                                return Err(rusqlite::Error::FromSqlConversionFailure(
                                    5,
                                    rusqlite::types::Type::Integer,
                                    Box::new(std::io::Error::other(
                                        "search encoded bytes budget exceeded",
                                    )),
                                ));
                            }
                            prospective_bytes += candidate_bytes;
                            #[cfg(test)]
                            CANDIDATE_MATERIALIZATIONS.with(|n| n.set(n.get() + 1));
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, Option<String>>(1)?,
                                r.get::<_, Option<String>>(2)?,
                                r.get::<_, String>(3)?,
                                r.get::<_, f64>(4)?,
                                estimate,
                            ))
                        },
                    )?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| match e {
                        rusqlite::Error::FromSqlConversionFailure(_, _, source)
                            if source.to_string() == "search encoded bytes budget exceeded" =>
                        {
                            Error::ReadBudgetExceeded {
                                kind: "search encoded bytes",
                                limit: 8 * 1024 * 1024 - 64 * 1024,
                            }
                        }
                        other => Error::Database(other),
                    })?;
                total_bytes = prospective_bytes;
                if rows.is_empty() {
                    break;
                }
                for (id, node, msg, snip, score, _estimate) in rows {
                    scanned += 1;
                    cursor_score = Some(score);
                    cursor_conversation = id.clone();
                    cursor_node = node.clone();
                    if !score.is_finite() {
                        continue;
                    }
                    let graph = if let Some(g) = cache.get(&id) {
                        g.clone()
                    } else {
                        let count:i64=tx.query_row("SELECT count(*) FROM (SELECT 1 FROM graph_nodes WHERE conversation_id=?1 LIMIT 100001)",[&id],|r|r.get(0))?;
                        total = total.saturating_add(count as usize);
                        if count > 100000 || total > 100000 {
                            return Err(Error::ReadBudgetExceeded {
                                kind: "search graph nodes",
                                limit: 100_000,
                            });
                        }
                        // Search only needs graph identity and parent edges. Do not
                        // hydrate or budget transcript parts/content/author payloads.
                        let bytes:i64=tx.query_row("SELECT coalesce(sum(length(CAST(id AS BLOB))+length(CAST(provider AS BLOB))+length(CAST(account_scope AS BLOB))+length(CAST(source_id AS BLOB))+length(CAST(title AS BLOB))+coalesce(length(CAST(url AS BLOB)),0)+coalesce(length(CAST(current_node AS BLOB)),0)),0) FROM conversations WHERE id=?1",[&id],|r|r.get(0))?;
                        let bytes=bytes.saturating_add(tx.query_row("SELECT coalesce(sum(length(CAST(source_node_id AS BLOB))+coalesce(length(CAST(parent_id AS BLOB)),0)),0) FROM graph_nodes WHERE conversation_id=?1",[&id],|r|r.get::<_,i64>(0))?);
                        let graph_bytes = (bytes.max(0) as usize)
                            .saturating_mul(6)
                            .saturating_add((count.max(0) as usize).saturating_mul(512));
                        total_bytes = total_bytes.saturating_add(graph_bytes);
                        if total_bytes > 8 * 1024 * 1024 - 64 * 1024 {
                            return Err(Error::ReadBudgetExceeded {
                                kind: "search encoded bytes",
                                limit: 8 * 1024 * 1024 - 64 * 1024,
                            });
                        }
                        let g = read_search_topology_conn(&tx, &id)?.unwrap_or_default();
                        cache.insert(id.clone(), g.clone());
                        g
                    };
                    let leaf = if let Some(ref n) = node {
                        let mut frontier = vec![n.clone()];
                        let mut visited = std::collections::HashSet::new();
                        let mut leaves = Vec::new();
                        while let Some(c) = frontier.pop() {
                            if !visited.insert(c.clone()) {
                                continue;
                            }
                            let children = graph.children.get(&c).cloned().unwrap_or_default();
                            if children.is_empty() {
                                leaves.push(c)
                            } else {
                                frontier.extend(children)
                            }
                        }
                        let selected_leaf = graph.selected_node_id.as_deref().filter(|selected| {
                            !graph.children.contains_key(*selected) && {
                                let mut cur = Some(selected.to_string());
                                let mut seen = std::collections::HashSet::new();
                                let mut found = false;
                                while let Some(c) = cur {
                                    if !seen.insert(c.clone()) {
                                        break;
                                    }
                                    if c == *n {
                                        found = true;
                                        break;
                                    }
                                    cur = graph.parents.get(&c).cloned().flatten();
                                }
                                found
                            }
                        });
                        selected_leaf
                            .map(str::to_owned)
                            .or_else(|| leaves.into_iter().min())
                    } else {
                        None
                    };
                    if node.is_some() && leaf.is_none() {
                        continue;
                    }
                    let selected = node
                        .as_ref()
                        .is_some_and(|n| graph.selected_thread.contains(n));
                    let summary: Value=tx.query_row("SELECT json_object('id',c.id,'provider',c.provider,'account_scope',c.account_scope,'source_id',c.source_id,'title',c.title,'updated_at_ms',c.updated_at_ms,'project',(SELECT json_object('project_key',p.project_key,'project_id',p.source_id,'title',p.title,'display_name',CASE WHEN trim(coalesce(p.local_name,''))<>'' THEN trim(p.local_name) WHEN trim(coalesce(p.title,''))<>'' THEN trim(p.title) ELSE NULL END,'name_source',CASE WHEN trim(coalesce(p.local_name,''))<>'' THEN 'local' WHEN trim(coalesce(p.title,''))<>'' THEN 'upstream' ELSE 'unknown' END) FROM conversation_projects cp JOIN projects p ON p.project_key=cp.project_key WHERE cp.conversation_id=c.id)) FROM conversations c WHERE c.id=?1",[&id],|r|r.get::<_,String>(0))?.parse().unwrap_or(Value::Null);
                    hits.push(serde_json::json!({"conversation":summary,"node_id":node,"message_id":msg,"leaf_node_id":leaf,"snippet":snip,"score":score,"selected_branch":selected}));
                    if serde_json::to_vec(&hits)
                        .map_or(true, |v| v.len() > 8 * 1024 * 1024 - 64 * 1024)
                    {
                        return Err(Error::ReadBudgetExceeded {
                            kind: "search encoded bytes",
                            limit: 8 * 1024 * 1024 - 64 * 1024,
                        });
                    }
                }
            }
            if scanned >= 1024 && hits.len() < limit.min(101) {
                return Err(Error::ReadBudgetExceeded {
                    kind: "search_scan_candidates",
                    limit: 1024,
                });
            }
        }
        tx.commit()?;
        Ok(SearchPageSnapshot {
            library_revision: revision,
            hits,
        })
    }
    pub fn search(&self, q: &str, limit: usize) -> Result<Vec<String>> {
        // Treat input as literal terms, never as FTS5 query syntax. Quoting each
        // whitespace-delimited term preserves AND-style matching without allowing
        // operators, column selectors, or embedded quotes to alter the query.
        let terms = literal_search_terms(q);
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
        tx.execute("DELETE FROM message_fts", [])?;
        tx.execute("INSERT INTO message_fts SELECT g.conversation_id,g.source_node_id,g.message_id,g.content FROM graph_nodes g WHERE g.role IS NOT NULL OR g.source_message_id IS NOT NULL", [])?;
        tx.execute("DELETE FROM title_fts", [])?;
        tx.execute(
            "INSERT INTO title_fts SELECT id,title FROM conversations",
            [],
        )?;
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
        Ok(match self.ingest_observation_outcome(o)? {
            IngestOutcome::Imported { conversation_id }
            | IngestOutcome::Updated { conversation_id }
            | IngestOutcome::Unchanged { conversation_id } => conversation_id,
        })
    }
    pub fn ingest_observation_outcome(&mut self, o: &Observation) -> Result<IngestOutcome> {
        validate(o)?;
        let tx = self.connection.transaction()?;
        let prior_revision: i64 = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let existed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM conversations WHERE provider=?1 AND account_scope=?2 AND source_id=?3)", params![o.provider, o.account_scope, o.source_id], |r| r.get(0))?;
        ingest_tx(&tx, o)?;
        let id = tx.query_row(
            "SELECT id FROM conversations WHERE provider=?1 AND account_scope=?2 AND source_id=?3",
            params![o.provider, o.account_scope, o.source_id],
            |r| r.get(0),
        )?;
        let after_revision: i64 = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(if after_revision == prior_revision {
            IngestOutcome::Unchanged {
                conversation_id: id,
            }
        } else if existed {
            IngestOutcome::Updated {
                conversation_id: id,
            }
        } else {
            IngestOutcome::Imported {
                conversation_id: id,
            }
        })
    }
}
#[derive(Clone, Debug, Default)]
struct SearchTopology {
    parents: std::collections::HashMap<String, Option<String>>,
    children: std::collections::HashMap<String, Vec<String>>,
    selected_node_id: Option<String>,
    selected_thread: std::collections::HashSet<String>,
}

/// Minimal graph projection used by search. This intentionally excludes all
/// transcript and presentation fields so large local messages do not block hits.
fn read_search_topology_conn(conn: &Connection, id: &str) -> Result<Option<SearchTopology>> {
    let Some(current) = conn
        .query_row(
            "SELECT current_node FROM conversations WHERE id=?1",
            [id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
    else {
        return Ok(None);
    };

    let mut topology = SearchTopology::default();
    let mut statement = conn.prepare(
        "SELECT source_node_id,parent_id FROM graph_nodes WHERE conversation_id=?1 ORDER BY source_node_id",
    )?;
    let rows = statement.query_map([id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (node, parent) = row?;
        if let Some(parent_id) = &parent {
            topology
                .children
                .entry(parent_id.clone())
                .or_default()
                .push(node.clone());
        }
        topology.parents.insert(node, parent);
    }

    topology.selected_node_id = current
        .filter(|selected| topology.parents.contains_key(selected))
        .or_else(|| {
            topology
                .parents
                .keys()
                .filter(|node| !topology.children.contains_key(*node))
                .min()
                .cloned()
        });
    let mut ancestry = Vec::new();
    let mut cursor = topology.selected_node_id.clone();
    while let Some(node) = cursor {
        if ancestry.contains(&node) {
            ancestry.clear();
            break;
        }
        ancestry.push(node.clone());
        cursor = topology
            .parents
            .get(&node)
            .cloned()
            .flatten()
            .filter(|parent| topology.parents.contains_key(parent));
    }
    topology.selected_thread.extend(ancestry);
    Ok(Some(topology))
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
            serde_json::json!({"id":nid,"source_id":nid,"parent_id":parent,"message_id":mid}),
        );
        if let (Some(mid), Some(role)) = (mid, role) {
            messages.push(serde_json::json!({"id":mid,"source_id":sid,"node_id":nid,"role":role,"author":author.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"create_time":time.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"created_at_ms":millis,"content":content,"parts":p}));
        }
    }
    let reason = if nodes.is_empty() {
        "empty"
    } else if current
        .as_ref()
        .is_some_and(|c| nodes.iter().any(|n| n["id"] == *c))
    {
        "source_current_node"
    } else {
        "fallback_leaf"
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
    let reason = if selected.is_none() { "empty" } else { reason };
    let mut ancestry = Vec::new();
    let mut cursor = selected.clone();
    let mut reached_root = selected.is_none() && nodes.is_empty();
    while let Some(c) = cursor {
        if ancestry.contains(&c) {
            ancestry.clear();
            break;
        }
        ancestry.push(c.clone());
        let parent = nodes
            .iter()
            .find(|n| n["id"] == c)
            .and_then(|n| n["parent_id"].as_str().map(str::to_owned));
        if parent.is_none() {
            reached_root = true;
        }
        cursor = parent.filter(|parent| nodes.iter().any(|n| n["id"] == *parent));
    }
    ancestry.reverse();
    Ok(Some(
        serde_json::json!({"id":id,"title":title,"provider":provider,"account_scope":scope,"scope":scope,"source_id":source,"url":url,"updated_at_ms":updated,"source_current_node_id":current,"selected_node_id":selected,"selection_reason":reason,"thread_node_ids":ancestry,"thread_complete":reached_root,"nodes":nodes,"messages":messages}),
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

fn literal_search_terms(raw_query: &str) -> Vec<String> {
    let mut terms: Vec<String> = raw_query
        .split_whitespace()
        .filter(|term| term.chars().any(char::is_alphanumeric))
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect();
    if let Some(last) = terms.last_mut() {
        last.push('*');
    }
    terms
}

fn project_key(provider: &str, account_scope: &str, source_id: &str) -> String {
    format!(
        "project:{}",
        serde_json::json!([provider, account_scope, source_id])
    )
}

fn observed_account_scope(scope: &str) -> bool {
    scope
        .strip_prefix("chatgpt:observed:")
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

fn is_chatgpt_provider(provider: &str) -> bool {
    matches!(provider, "chatgpt" | "paseo-chatgpt")
}

fn valid_upstream_conversation_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn valid_upstream_project_id(id: &str) -> bool {
    id.strip_prefix("g-p-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() <= 128
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    })
}

fn project_display_name(
    local_name: Option<&str>,
    upstream_name: Option<&str>,
) -> (Option<String>, &'static str) {
    if let Some(name) = local_name.map(str::trim).filter(|value| !value.is_empty()) {
        (Some(name.to_owned()), "local")
    } else if let Some(name) = upstream_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        (Some(name.to_owned()), "upstream")
    } else {
        (None, "unknown")
    }
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
type PriorState = (
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);
fn canonical_ordering_payload(o: &Observation) -> String {
    let mut graph = o
        .graph
        .iter()
        .map(|n| {
            serde_json::json!([
                n.source_node_id,
                n.source_message_id,
                n.parent_id,
                n.role,
                n.author,
                n.create_time,
                n.parts
            ])
        })
        .collect::<Vec<_>>();
    graph.sort_by_key(Value::to_string);
    serde_json::json!([o.title, o.current_node, graph]).to_string()
}

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
            "SELECT title,url,current_node,last_source_updated_ms,ordering_observed_ms FROM conversations WHERE id=?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let prior_ms = prior.as_ref().and_then(|x| x.3);
    let prior_observed_ms = prior.as_ref().and_then(|x| x.4);
    let mut ordering_observation = o.clone();
    if ordering_observation.url.is_none() {
        ordering_observation.url = prior.as_ref().and_then(|p| p.1.clone());
    }
    let incoming_key = canonical_ordering_payload(&ordering_observation);
    let prior_key: Option<String> = tx
        .query_row(
            "SELECT ordering_payload FROM conversations WHERE id=?1",
            [&id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let accept = match (source_ms, prior_ms) {
        (Some(new), Some(old)) if new < old => false,
        (None, Some(_)) => false,
        (Some(new), Some(old)) if new == old => prior_key
            .as_ref()
            .is_none_or(|old_key| incoming_key > *old_key),
        (None, None) => match prior_key.as_ref() {
            None => true,
            Some(old_key) => {
                o.observed_ms > prior_observed_ms.unwrap_or(i64::MIN)
                    || (o.observed_ms == prior_observed_ms.unwrap_or(i64::MIN)
                        && incoming_key > *old_key)
            }
        },
        (Some(_), None) => true,
        _ => true,
    };
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
        tx.execute("INSERT INTO conversations(id,title,provider,account_scope,source_id,url,current_node,last_source_updated_ms,ordering_payload,ordering_observed_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(provider,account_scope,source_id) DO UPDATE SET title=CASE WHEN excluded.title='' THEN conversations.title ELSE excluded.title END,url=coalesce(excluded.url,conversations.url),current_node=excluded.current_node,last_source_updated_ms=coalesce(excluded.last_source_updated_ms,conversations.last_source_updated_ms),ordering_payload=excluded.ordering_payload,ordering_observed_ms=excluded.ordering_observed_ms",params![id,o.title,o.provider,o.account_scope,o.source_id,o.url,o.current_node,source_ms,incoming_key,o.observed_ms])?;
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
    let metadata_changed = source_ms.is_some() && source_ms != prior_ms && prior.is_some();
    let visible_changed = prior.as_ref().map(|p| (&p.0, &p.1, &p.2))
        != Some((&persisted.0, &persisted.1, &persisted.2))
        || before_graph != after_graph;
    let changed = accept
        && (metadata_changed
            || (visible_changed
                && (source_ms.is_none() || prior.is_none() || source_ms >= prior_ms)));
    let changed =
        changed || (accept && visible_changed && source_ms.is_some() && source_ms == prior_ms);
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
mod pre_materialization_tests {
    use super::*;

    #[test]
    fn numeric_search_budget_preflights_before_candidate_strings() {
        let mut store = Store::in_memory().unwrap();
        let huge_title = format!("probe {}", "x".repeat(2 * 1024 * 1024));
        let observation = Observation {
            provider: "p".into(),
            account_scope: "a".into(),
            source_id: "huge".into(),
            title: huge_title,
            url: None,
            current_node: None,
            graph: vec![],
            raw_payload: Value::Null,
            source_time: None,
            observed_ms: 1,
            parser: "test".into(),
            completeness: "complete".into(),
        };
        store.ingest_observation(&observation).unwrap();
        CANDIDATE_MATERIALIZATIONS.with(|n| n.set(0));
        let result = store.search_page_snapshot("probe", 10, None, None);
        assert!(
            matches!(result, Err(Error::ReadBudgetExceeded { .. })),
            "unexpected result: {result:?}"
        );
        CANDIDATE_MATERIALIZATIONS.with(|n| assert_eq!(n.get(), 0));

        store
            .connection
            .execute("DELETE FROM conversations WHERE id='huge'", [])
            .unwrap();
        store.connection.execute(
            "INSERT INTO conversations(id,title,provider,account_scope,source_id) VALUES('small','needle','p','a','s')",
            [],
        ).unwrap();
        store
            .connection
            .execute("INSERT INTO title_fts VALUES('small','needle')", [])
            .unwrap();
        CANDIDATE_MATERIALIZATIONS.with(|n| n.set(0));
        let control = store.search_page_snapshot("needle", 10, None, None);
        assert!(control.is_ok(), "small candidate query: {control:?}");
        CANDIDATE_MATERIALIZATIONS.with(|n| assert!(n.get() > 0));
    }

    #[test]
    fn cumulative_search_budget_stops_before_second_candidate_strings() {
        let store = Store::in_memory().unwrap();
        let title = format!("common {}", "x ".repeat(300 * 1024));
        for id in ["first", "second"] {
            store.connection.execute(
                "INSERT INTO conversations(id,title,provider,account_scope,source_id) VALUES(?1,?2,'p','a',?1)",
                params![id, title],
            ).unwrap();
            store
                .connection
                .execute("INSERT INTO title_fts VALUES(?1,?2)", params![id, title])
                .unwrap();
        }
        CANDIDATE_MATERIALIZATIONS.with(|n| n.set(0));
        let result = store.search_page_snapshot("common", 2, None, None);
        assert!(
            matches!(
                result,
                Err(Error::ReadBudgetExceeded {
                    kind: "search encoded bytes",
                    ..
                })
            ),
            "unexpected result: {result:?}"
        );
        CANDIDATE_MATERIALIZATIONS.with(|n| assert_eq!(n.get(), 1, "result: {result:?}"));
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn migrated_legacy_ids_and_rows_survive_ingest() {
        let mut store = Store::in_memory().unwrap();
        store.connection.execute_batch("DROP TABLE IF EXISTS conversation_projects; DROP TABLE IF EXISTS projects; DROP TRIGGER IF EXISTS message_fts_insert; DROP TRIGGER IF EXISTS message_fts_delete; DROP TRIGGER IF EXISTS message_fts_update; DROP TRIGGER IF EXISTS title_fts_insert; DROP TRIGGER IF EXISTS title_fts_update; DROP TRIGGER IF EXISTS title_fts_delete; DROP TABLE IF EXISTS message_fts; DROP TABLE IF EXISTS title_fts; DROP TABLE IF EXISTS library_state; DROP TABLE normalized_fts; DROP TABLE conversations_fts; DROP INDEX conversations_identity; DROP TRIGGER normalized_conv_insert; DROP TRIGGER normalized_conv_update; DROP TRIGGER normalized_conv_delete; DROP TRIGGER normalized_graph_insert; DROP TRIGGER normalized_graph_update; DROP TRIGGER normalized_graph_delete; DROP TABLE graph_nodes; DROP TABLE observations; DROP TABLE messages; DROP TABLE conversations; CREATE TABLE conversations(id TEXT PRIMARY KEY,title TEXT NOT NULL DEFAULT '',provider TEXT NOT NULL DEFAULT '',url TEXT,updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); CREATE TABLE messages(id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id),parent_id TEXT,role TEXT NOT NULL,content TEXT NOT NULL DEFAULT ''); INSERT INTO conversations(id,title,provider) VALUES('legacy-conv','old','legacy'); INSERT INTO messages(id,conversation_id,parent_id,role,content) VALUES('legacy-msg','legacy-conv',NULL,'user','kept'); DELETE FROM schema_migrations WHERE version>=3;").unwrap();
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

#[cfg(test)]
mod project_and_search_tests {
    use super::*;

    fn conversation(store: &Store, id: &str, provider: &str, title: &str) {
        store.connection.execute(
            "INSERT INTO conversations(id,title,provider,account_scope,source_id,updated_at_ms) VALUES(?1,?2,?3,'scope',?1,1)",
            params![id,title,provider],
        ).unwrap();
    }

    #[test]
    fn source_project_filters_and_last_token_prefix_are_applied_before_paging() {
        let mut store = Store::in_memory().unwrap();
        conversation(&store, "chat_project", "chatgpt", "alpha beta");
        conversation(&store, "chat_other", "chatgpt", "alpha gamma");
        conversation(&store, "codex", "codex_local", "alpha beta");
        store
            .apply_project_memberships(
                "chatgpt",
                "scope",
                &[ProjectMembership {
                    source_id: "chat_project".into(),
                    project_id: Some("gizmo-1".into()),
                    title: Some("Research".into()),
                }],
            )
            .unwrap();
        let projects = store.projects_snapshot("chatgpt").unwrap();
        assert_eq!(projects.projects.len(), 1);
        assert_eq!(projects.projects[0]["title"], "Research");
        assert_eq!(projects.projects[0]["conversation_count"], 1);
        let key = projects.projects[0]["project_key"].as_str().unwrap();
        let page = store
            .source_list_page_snapshot(10, None, None, "chatgpt", Some(key))
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0]["id"], "chat_project");
        assert_eq!(page.items[0]["project"]["title"], "Research");
        let codex = store
            .source_list_page_snapshot(10, None, None, "codex", None)
            .unwrap();
        assert_eq!(codex.items.len(), 1);
        assert_eq!(codex.items[0]["id"], "codex");
        let hits = store
            .source_search_page_snapshot("alpha be", 10, None, None, "chatgpt", Some(key))
            .unwrap();
        assert_eq!(hits.hits.len(), 1);
        assert_eq!(hits.hits[0]["conversation"]["id"], "chat_project");
    }

    #[test]
    fn project_metadata_rejects_unknown_targets_without_partial_update() {
        let mut store = Store::in_memory().unwrap();
        conversation(&store, "known", "chatgpt", "alpha");
        let result = store.apply_project_memberships(
            "chatgpt",
            "scope",
            &[
                ProjectMembership {
                    source_id: "known".into(),
                    project_id: Some("p".into()),
                    title: Some("Title".into()),
                },
                ProjectMembership {
                    source_id: "missing".into(),
                    project_id: Some("q".into()),
                    title: None,
                },
            ],
        );
        assert!(result.is_err());
        assert_eq!(
            store.projects_snapshot("chatgpt").unwrap().projects.len(),
            0
        );
    }

    #[test]
    fn verified_sidebar_titles_update_matching_provisional_ids_only() {
        let mut store = Store::in_memory().unwrap();
        let current_scope = format!("chatgpt:observed:{}", "a".repeat(64));
        let other_scope = format!("chatgpt:observed:{}", "b".repeat(64));
        for (key, scope, source_id, title) in [
            (
                "legacy",
                "provisional:default",
                "gizmo-shared",
                None::<String>,
            ),
            (
                "cached-legacy",
                "paseo-cache:owner-legacy",
                "gizmo-shared",
                None::<String>,
            ),
            (
                "current",
                current_scope.as_str(),
                "gizmo-shared",
                None::<String>,
            ),
            (
                "other",
                other_scope.as_str(),
                "gizmo-shared",
                None::<String>,
            ),
            (
                "unrelated",
                "provisional:default",
                "gizmo-other",
                None::<String>,
            ),
        ] {
            store.connection.execute(
                "INSERT INTO projects(project_key,provider,account_scope,source_id,title) VALUES(?1,'chatgpt',?2,?3,?4)",
                params![key, scope, source_id, title],
            ).unwrap();
        }
        store
            .apply_project_titles(
                "chatgpt",
                &current_scope,
                &[ProjectTitle {
                    source_id: "gizmo-shared".into(),
                    title: Some("Design work".into()),
                }],
            )
            .unwrap();
        let titles = ["legacy", "cached-legacy", "current", "other", "unrelated"]
            .into_iter()
            .map(|key| {
                let title: Option<String> = store
                    .connection
                    .query_row(
                        "SELECT title FROM projects WHERE project_key=?1",
                        [key],
                        |row| row.get(0),
                    )
                    .unwrap();
                (key, title)
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(titles["legacy"].as_deref(), Some("Design work"));
        assert_eq!(titles["cached-legacy"].as_deref(), Some("Design work"));
        assert_eq!(titles["current"].as_deref(), Some("Design work"));
        assert_eq!(titles["other"], None);
        assert_eq!(titles["unrelated"], None);
    }

    #[test]
    fn verified_sidebar_titles_update_legacy_paseo_provider_without_relabeling_it() {
        let mut store = Store::in_memory().unwrap();
        let current_scope = format!("chatgpt:observed:{}", "c".repeat(64));
        store.connection.execute(
            "INSERT INTO projects(project_key,provider,account_scope,source_id,title) VALUES('legacy-paseo-project','paseo-chatgpt','paseo-cache:legacy-owner','g-p-legacy-project',NULL)",
            [],
        ).unwrap();
        store.apply_project_titles(
            "chatgpt",
            &current_scope,
            &[ProjectTitle { source_id: "g-p-legacy-project".into(), title: Some("Verified sidebar name".into()) }],
        ).unwrap();
        let record: (String, String, Option<String>) = store.connection.query_row(
            "SELECT provider,account_scope,title FROM projects WHERE project_key='legacy-paseo-project'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(record, ("paseo-chatgpt".into(), "paseo-cache:legacy-owner".into(), Some("Verified sidebar name".into())));
    }

    #[test]
    fn verified_sidebar_title_updates_project_created_by_legacy_migration() {
        let mut store = Store::in_memory().unwrap();
        let source_id = "g-p-migrated-project";
        store.connection.execute(
            "INSERT INTO conversations(id,title,provider,account_scope,source_id,updated_at_ms) VALUES('legacy-chat','Legacy chat','paseo-chatgpt','paseo-cache:legacy-owner','legacy-chat',1)",
            [],
        ).unwrap();
        store.connection.execute(
            "INSERT INTO observations(conversation_id,observed_ms,parser,completeness,raw_payload) VALUES('legacy-chat',1,'legacy-fixture','complete',?1)",
            [serde_json::json!({"paseo_cache":{"project":source_id}}).to_string()],
        ).unwrap();
        store.connection.execute_batch(
            "DROP TABLE conversation_projects; DROP TABLE projects; DELETE FROM schema_migrations WHERE version>=9;",
        ).unwrap();
        store.migrate().unwrap();

        store.apply_project_titles(
            "chatgpt",
            &format!("chatgpt:observed:{}", "d".repeat(64)),
            &[ProjectTitle { source_id: source_id.into(), title: Some("Migrated project name".into()) }],
        ).unwrap();
        let projects = store.projects_snapshot("chatgpt").unwrap();
        assert_eq!(projects.projects.len(), 1);
        assert_eq!(projects.projects[0]["title"], "Migrated project name");
        assert_eq!(projects.projects[0]["project_id"], source_id);
        assert_eq!(projects.projects[0]["conversation_count"], 1);
        let identity: (String, String) = store.connection.query_row(
            "SELECT provider,account_scope FROM projects WHERE source_id=?1", [source_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(identity, ("paseo-chatgpt".into(), "paseo-cache:legacy-owner".into()));
    }

    #[test]
    fn local_project_name_overrides_upstream_without_changing_project_identity() {
        let mut store = Store::in_memory().unwrap();
        conversation(&store, "named_chat", "chatgpt", "alpha");
        store
            .apply_project_memberships(
                "chatgpt",
                "scope",
                &[ProjectMembership {
                    source_id: "named_chat".into(),
                    project_id: Some("opaque-upstream-id".into()),
                    title: Some("  Upstream title  ".into()),
                }],
            )
            .unwrap();
        let project = store
            .projects_snapshot("chatgpt")
            .unwrap()
            .projects
            .remove(0);
        assert_eq!(project["display_name"], "Upstream title");
        assert_eq!(project["name_source"], "upstream");
        let key = project["project_key"].as_str().unwrap().to_owned();
        assert_eq!(project["project_id"], "opaque-upstream-id");

        let renamed = store.rename_project(&key, Some("  Local label  ")).unwrap();
        assert_eq!(renamed.project["display_name"], "Local label");
        assert_eq!(renamed.project["name_source"], "local");
        store
            .apply_project_memberships(
                "chatgpt",
                "scope",
                &[ProjectMembership {
                    source_id: "named_chat".into(),
                    project_id: Some("opaque-upstream-id".into()),
                    title: Some("Updated upstream title".into()),
                }],
            )
            .unwrap();
        let project = store
            .projects_snapshot("chatgpt")
            .unwrap()
            .projects
            .remove(0);
        assert_eq!(project["title"], "Updated upstream title");
        assert_eq!(project["display_name"], "Local label");
        assert_eq!(project["name_source"], "local");

        let cleared = store.rename_project(&key, Some("  ")).unwrap();
        assert_eq!(cleared.project["display_name"], "Updated upstream title");
        assert_eq!(cleared.project["name_source"], "upstream");
        assert_eq!(cleared.project["project_id"], "opaque-upstream-id");
    }

    #[test]
    fn unknown_project_names_remain_unknown_and_rename_validates_input() {
        let mut store = Store::in_memory().unwrap();
        conversation(&store, "unnamed_chat", "chatgpt", "alpha");
        store
            .apply_project_memberships(
                "chatgpt",
                "scope",
                &[ProjectMembership {
                    source_id: "unnamed_chat".into(),
                    project_id: Some("g-opaque-id".into()),
                    title: None,
                }],
            )
            .unwrap();
        let project = store
            .projects_snapshot("chatgpt")
            .unwrap()
            .projects
            .remove(0);
        assert_eq!(project["display_name"], Value::Null);
        assert_eq!(project["name_source"], "unknown");
        let key = project["project_key"].as_str().unwrap();
        assert!(store.rename_project(key, Some(&"x".repeat(121))).is_err());
        assert!(store.rename_project(key, Some("bad\nname")).is_err());
        assert!(store.rename_project("missing", Some("name")).is_err());
    }

    #[test]
    fn project_assignment_context_allows_explicitly_verified_legacy_scope_without_relabeling_it() {
        let mut store = Store::in_memory().unwrap();
        let conversation_id = "123e4567-e89b-12d3-a456-426614174000";
        let project_id = format!("g-p-{}", "a".repeat(32));
        store.connection.execute(
            "INSERT INTO conversations(id,title,provider,account_scope,source_id,updated_at_ms) VALUES(?1,'Legacy chat','paseo-chatgpt','paseo-cache:owner',?1,1)",
            [conversation_id],
        ).unwrap();
        store.connection.execute(
            "INSERT INTO projects(project_key,provider,account_scope,source_id,title,local_name) VALUES('current-project','chatgpt','chatgpt:observed:'||?1,?2,'Old title','Local name')",
            params!["b".repeat(64), project_id],
        ).unwrap();
        let target = store
            .project_assignment_context(conversation_id, Some("current-project"))
            .unwrap();
        assert_eq!(target["conversation_source_id"], conversation_id);
        assert_eq!(target["conversation_scope"], "paseo-cache:owner");
        assert_eq!(target["project_id"], project_id);
        let receipt = store
            .commit_project_assignment(
                conversation_id,
                "paseo-cache:owner",
                Some("current-project"),
                Some("Verified current title"),
            )
            .unwrap();
        assert_eq!(receipt["project"]["display_name"], "Local name");
        let persisted_scope: String = store
            .connection
            .query_row(
                "SELECT account_scope FROM conversations WHERE id=?1",
                [conversation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(persisted_scope, "paseo-cache:owner");
        let upstream_title: String = store
            .connection
            .query_row(
                "SELECT title FROM projects WHERE project_key='current-project'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(upstream_title, "Verified current title");
        store
            .commit_project_assignment(conversation_id, "paseo-cache:owner", None, None)
            .unwrap();
        assert_eq!(
            store
                .connection
                .query_row::<i64, _, _>(
                    "SELECT count(*) FROM conversation_projects WHERE conversation_id=?1",
                    [conversation_id],
                    |row| row.get(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn project_assignment_rejects_observed_account_mismatch_and_nonupstream_ids() {
        let mut store = Store::in_memory().unwrap();
        let conversation_id = "123e4567-e89b-12d3-a456-426614174000";
        let scope_a = format!("chatgpt:observed:{}", "a".repeat(64));
        let scope_b = format!("chatgpt:observed:{}", "b".repeat(64));
        let project_id = format!("g-p-{}", "c".repeat(32));
        store.connection.execute(
            "INSERT INTO conversations(id,title,provider,account_scope,source_id,updated_at_ms) VALUES(?1,'Chat','chatgpt',?2,?1,1)",
            params![conversation_id,scope_a],
        ).unwrap();
        store.connection.execute(
            "INSERT INTO projects(project_key,provider,account_scope,source_id,title) VALUES('other-account','chatgpt',?1,?2,'Title')",
            params![scope_b,project_id],
        ).unwrap();
        assert!(store
            .project_assignment_context(conversation_id, Some("other-account"))
            .is_err());
        assert!(store
            .commit_project_assignment(conversation_id, &scope_a, Some("other-account"), None)
            .is_err());
        assert!(store
            .project_assignment_context("chat_project", Some("other-account"))
            .is_err());
    }

    #[test]
    fn project_migration_uses_only_latest_observation() {
        let mut store = Store::in_memory().unwrap();
        store.connection.execute_batch("DROP TABLE conversation_projects; DROP TABLE projects; DELETE FROM schema_migrations WHERE version>=9;").unwrap();
        store.connection.execute("INSERT INTO conversations(id,title,provider,account_scope,source_id) VALUES('legacy','cached','paseo-chatgpt','paseo-cache:owner','legacy')", []).unwrap();
        store.connection.execute("INSERT INTO observations(id,conversation_id,source_time,observed_ms,parser,completeness,raw_payload) VALUES(1,'legacy',NULL,1,'p','complete','{\"paseo_cache\":{\"project\":\"old\"}}'),(2,'legacy',NULL,2,'p','complete','{\"paseo_cache\":{\"project\":\"new\"}}')", []).unwrap();
        store.migrate().unwrap();
        let project_id: String = store
            .connection
            .query_row("SELECT source_id FROM projects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(project_id, "new");
        assert_eq!(
            store
                .connection
                .query_row::<i64, _, _>("SELECT count(*) FROM conversation_projects", [], |row| row
                    .get(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn project_migration_ignores_empty_ids_and_v10_cleans_existing_empty_memberships() {
        let mut store = Store::in_memory().unwrap();
        store.connection.execute_batch("DROP TABLE conversation_projects; DROP TABLE projects; DELETE FROM schema_migrations WHERE version>=9;").unwrap();
        store.connection.execute("INSERT INTO conversations(id,title,provider,account_scope,source_id) VALUES('legacy-empty','cached','paseo-chatgpt','paseo-cache:owner','legacy-empty')", []).unwrap();
        store.connection.execute("INSERT INTO observations(id,conversation_id,observed_ms,parser,completeness,raw_payload) VALUES(1,'legacy-empty',1,'p','complete','{\"paseo_cache\":{\"project\":\"\"}}')", []).unwrap();
        store.migrate().unwrap();
        assert_eq!(
            store
                .connection
                .query_row::<i64, _, _>("SELECT count(*) FROM projects", [], |row| row.get(0))
                .unwrap(),
            0
        );
        assert_eq!(store.schema_version().unwrap(), 11);

        store.connection.execute("INSERT INTO projects(project_key,provider,account_scope,source_id,title) VALUES('bad-empty','paseo-chatgpt','paseo-cache:owner','','Assigned project')", []).unwrap();
        store.connection.execute("INSERT INTO conversation_projects(conversation_id,project_key) VALUES('legacy-empty','bad-empty')", []).unwrap();
        store
            .connection
            .execute_batch("DELETE FROM schema_migrations WHERE version>=10; ALTER TABLE projects DROP COLUMN local_name;")
            .unwrap();
        store.migrate().unwrap();
        assert_eq!(
            store
                .connection
                .query_row::<i64, _, _>("SELECT count(*) FROM projects", [], |row| row.get(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .connection
                .query_row::<i64, _, _>("SELECT count(*) FROM conversation_projects", [], |row| row
                    .get(0))
                .unwrap(),
            0
        );
        assert_eq!(store.library_revision().unwrap(), 1);
    }

    #[test]
    fn title_matches_receive_relevance_boost_and_search_cursor_order_is_stable() {
        let store = Store::in_memory().unwrap();
        conversation(&store, "title_hit", "chatgpt", "focus phrase");
        conversation(&store, "body_hit", "chatgpt", "unrelated");
        store.connection.execute("INSERT INTO graph_nodes(conversation_id,source_node_id,source_message_id,message_id,parent_id,role,parts_json,content) VALUES('body_hit','node','message','message',NULL,'assistant','[]','focus phrase')", []).unwrap();
        let page = store
            .search_page_snapshot("focus phrase", 10, None, None)
            .unwrap();
        assert!(page.hits.len() >= 2);
        assert_eq!(page.hits[0]["conversation"]["id"], "title_hit");
        assert!(page.hits.windows(2).all(|pair| {
            let a = pair[0]["score"].as_f64().unwrap();
            let b = pair[1]["score"].as_f64().unwrap();
            a < b
                || (a == b
                    && pair[0]["conversation"]["id"].as_str()
                        <= pair[1]["conversation"]["id"].as_str())
        }));
    }
}
mod catalog;
pub(crate) mod operation_learner;
