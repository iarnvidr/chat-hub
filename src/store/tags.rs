use super::{Store, TagSnapshot, TaggedTargetSnapshot};
use crate::error::{Error, Result};
use crate::store::CHAT_IDENTITY_CTES;
use rusqlite::{params, OptionalExtension};
use std::collections::HashSet;

const MAX_TARGET_ID_CHARS: usize = 2048;
const MAX_TAG_CHARS: usize = 64;
const MAX_TAGS_PER_TARGET: usize = 40;
const MAX_TAG_QUERY_CHARS: usize = 64;

fn validate_target(target_kind: &str, target_id: &str) -> Result<()> {
    if !matches!(target_kind, "conversation" | "entity" | "file") {
        return Err(Error::Configuration("invalid tag target kind".into()));
    }
    if target_id.trim().is_empty()
        || target_id != target_id.trim()
        || target_id.chars().count() > MAX_TARGET_ID_CHARS
        || target_id.chars().any(char::is_control)
    {
        return Err(Error::Configuration("invalid tag target id".into()));
    }
    Ok(())
}

fn normalize_tag(tag: &str) -> Result<(String, String)> {
    if tag.chars().any(char::is_control) {
        return Err(Error::Configuration("invalid tag name".into()));
    }
    let name = tag.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > MAX_TAG_CHARS || name.chars().any(char::is_control)
    {
        return Err(Error::Configuration("invalid tag name".into()));
    }
    let key = name
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<String>();
    Ok((key, name))
}

pub(super) fn normalized_tag_key(tag: &str) -> Result<String> {
    normalize_tag(tag).map(|(key, _)| key)
}

fn validate_tags(tags: &[String]) -> Result<Vec<(String, String)>> {
    if tags.len() > MAX_TAGS_PER_TARGET {
        return Err(Error::Configuration("too many tags for target".into()));
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(tags.len());
    for tag in tags {
        let pair = normalize_tag(tag)?;
        if seen.insert(pair.0.clone()) {
            normalized.push(pair);
        }
    }
    Ok(normalized)
}

impl Store {
    /// Replace local tags for a stable conversation, catalog entity, or file ID.
    /// Empty `tags` clears the target. This writes only tag metadata.
    pub fn set_target_tags(
        &mut self,
        target_kind: &str,
        target_id: &str,
        tags: &[String],
    ) -> Result<TaggedTargetSnapshot> {
        validate_target(target_kind, target_id)?;
        let tags = validate_tags(tags)?;
        let tx = self.connection.transaction()?;
        if target_kind == "conversation"
            && tx
                .query_row(
                    "SELECT 1 FROM conversations WHERE id=?1",
                    [target_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_none()
        {
            return Err(Error::Configuration("unknown tag target".into()));
        }
        let (storage_id, related_ids) = if target_kind == "conversation" {
            let ids = conversation_identity_ids(&tx, target_id)?;
            (ids[0].clone(), ids)
        } else {
            (target_id.to_owned(), vec![target_id.to_owned()])
        };
        let requested_keys = tags.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>();
        let canonical_keys = read_target_tag_keys(&tx, target_kind, &storage_id)?;
        let mut aliases_have_tags = false;
        for id in related_ids.iter().skip(1) {
            aliases_have_tags |= !read_target_tag_keys(&tx, target_kind, id)?.is_empty();
        }
        if canonical_keys == requested_keys && !aliases_have_tags {
            let mut saved = read_target_tags(&tx, target_kind, &storage_id)?;
            saved.target_id = target_id.to_owned();
            saved.tag_revision = current_tag_revision(&tx)?;
            tx.commit()?;
            return Ok(saved);
        }
        tx.execute(
            "DELETE FROM target_tags WHERE target_kind=?1 AND target_id IN (SELECT value FROM json_each(?2))",
            params![target_kind, serde_json::to_string(&related_ids).map_err(|e| Error::Configuration(e.to_string()))?],
        )?;
        for (ordinal, (key, name)) in tags.iter().enumerate() {
            tx.execute(
                "INSERT INTO tags(name_key,name) VALUES(?1,?2) ON CONFLICT(name_key) DO NOTHING",
                params![key, name],
            )?;
            tx.execute(
                "INSERT INTO target_tags(target_kind,target_id,name_key,ordinal) VALUES(?1,?2,?3,?4)",
                params![target_kind, storage_id, key, ordinal as i64],
            )?;
        }
        tx.execute("DELETE FROM tags WHERE NOT EXISTS(SELECT 1 FROM target_tags WHERE target_tags.name_key=tags.name_key)", [])?;
        tx.execute(
            "UPDATE tag_state SET revision=revision+1 WHERE singleton=1",
            [],
        )?;
        let mut saved = read_target_tags(&tx, target_kind, &storage_id)?;
        saved.target_id = target_id.to_owned();
        saved.tag_revision = current_tag_revision(&tx)?;
        tx.commit()?;
        Ok(saved)
    }

    pub fn target_tags(&self, target_kind: &str, target_id: &str) -> Result<Vec<String>> {
        validate_target(target_kind, target_id)?;
        if target_kind == "conversation" {
            let ids = conversation_identity_ids(&self.connection, target_id)?;
            return read_target_tags_for_ids(&self.connection, target_kind, &ids);
        }
        let mut stmt = self.connection.prepare(
            "SELECT t.name FROM target_tags x JOIN tags t ON t.name_key=x.name_key WHERE x.target_kind=?1 AND x.target_id=?2 ORDER BY x.ordinal,t.name_key",
        )?;
        let values = stmt
            .query_map(params![target_kind, target_id], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(values)
    }

    /// Return the local tag vocabulary and the number of targets using each tag.
    pub fn list_tags(
        &self,
        target_kind: Option<&str>,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TagSnapshot>> {
        if target_kind.is_some_and(|kind| !matches!(kind, "conversation" | "entity" | "file")) {
            return Err(Error::Configuration("invalid tag target kind".into()));
        }
        if !(1..=500).contains(&limit) {
            return Err(Error::Configuration("invalid tag list limit".into()));
        }
        let query = query.map(str::trim).filter(|value| !value.is_empty());
        if query.is_some_and(|value| {
            value.chars().count() > MAX_TAG_QUERY_CHARS || value.chars().any(char::is_control)
        }) {
            return Err(Error::Configuration("invalid tag query".into()));
        }
        let query = query.map(|value| {
            value
                .chars()
                .flat_map(char::to_lowercase)
                .collect::<String>()
        });
        let mut stmt = self.connection.prepare(&format!(
            "{CHAT_IDENTITY_CTES} SELECT t.name,count(DISTINCT x.target_kind || ':' || CASE WHEN x.target_kind='conversation' THEN coalesce(ca.canonical_conversation_id,x.target_id) ELSE x.target_id END) FROM tags t JOIN target_tags x ON x.name_key=t.name_key LEFT JOIN conversation_aliases ca ON x.target_kind='conversation' AND ca.legacy_conversation_id=x.target_id WHERE (?1 IS NULL OR x.target_kind=?1) AND (?2 IS NULL OR instr(t.name_key,?2)>0) GROUP BY t.name_key,t.name ORDER BY t.name_key LIMIT ?3"
        ))?;
        let values = stmt
            .query_map(params![target_kind, query, limit as i64], |row| {
                Ok(TagSnapshot {
                    name: row.get(0)?,
                    target_count: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(values)
    }

    /// Page stable target IDs carrying one exact tag. The optional cursor is
    /// the last `(target_kind,target_id)` pair returned by the previous page.
    pub fn tagged_targets(
        &self,
        target_kind: Option<&str>,
        tag: &str,
        limit: usize,
        after: Option<(&str, &str)>,
    ) -> Result<Vec<TaggedTargetSnapshot>> {
        Ok(self
            .tagged_targets_snapshot(target_kind, tag, limit, after)?
            .items)
    }

    pub fn tagged_targets_snapshot(
        &self,
        target_kind: Option<&str>,
        tag: &str,
        limit: usize,
        after: Option<(&str, &str)>,
    ) -> Result<crate::store::TaggedTargetsPage> {
        if target_kind.is_some_and(|kind| !matches!(kind, "conversation" | "entity" | "file")) {
            return Err(Error::Configuration("invalid tag target kind".into()));
        }
        if !(1..=1000).contains(&limit) {
            return Err(Error::Configuration("invalid tagged target limit".into()));
        }
        let (tag_key, _) = normalize_tag(tag)?;
        if let Some((kind, id)) = after {
            validate_target(kind, id)
                .map_err(|_| Error::Configuration("invalid tagged target cursor".into()))?;
        }
        let (after_kind, after_id) = after.unwrap_or(("", ""));
        let tx = self.connection.unchecked_transaction()?;
        let tag_revision: i64 = tx.query_row(
            "SELECT revision FROM tag_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let mut stmt = tx.prepare(&format!(
            "{CHAT_IDENTITY_CTES}, matching AS (
               SELECT x.target_kind,
                      CASE WHEN x.target_kind='conversation' THEN coalesce(ca.canonical_conversation_id,x.target_id) ELSE x.target_id END AS target_id,
                      x.target_id AS physical_id
               FROM target_tags x
               LEFT JOIN conversation_aliases ca ON x.target_kind='conversation' AND ca.legacy_conversation_id=x.target_id
               WHERE x.name_key=?1 AND (?2 IS NULL OR x.target_kind=?2)
            ), target_page AS (
               SELECT target_kind,target_id FROM matching
               WHERE (?3=0 OR target_kind>?4 OR (target_kind=?4 AND target_id>?5))
               GROUP BY target_kind,target_id ORDER BY target_kind,target_id LIMIT ?6
            )
            SELECT p.target_kind,p.target_id,t.name
            FROM target_page p JOIN matching m ON m.target_kind=p.target_kind AND m.target_id=p.target_id
            JOIN target_tags x ON x.target_kind=m.target_kind AND x.target_id=m.physical_id
            JOIN tags t ON t.name_key=x.name_key
            GROUP BY p.target_kind,p.target_id,t.name_key,t.name
            ORDER BY p.target_kind,p.target_id,min(x.ordinal),t.name_key"
        ))?;
        let mut rows = stmt.query(params![
            tag_key,
            target_kind,
            if after.is_some() { 1_i64 } else { 0_i64 },
            after_kind,
            after_id,
            limit as i64
        ])?;
        let mut targets = Vec::<TaggedTargetSnapshot>::new();
        while let Some(row) = rows.next()? {
            let target_kind: String = row.get(0)?;
            let target_id: String = row.get(1)?;
            let name: String = row.get(2)?;
            if targets.last().is_none_or(|target| {
                target.target_kind != target_kind || target.target_id != target_id
            }) {
                targets.push(TaggedTargetSnapshot {
                    target_kind,
                    target_id,
                    tags: Vec::new(),
                    tag_revision,
                });
            }
            targets.last_mut().expect("target pushed").tags.push(name);
        }
        drop(rows);
        drop(stmt);
        tx.commit()?;
        Ok(crate::store::TaggedTargetsPage {
            tag_revision,
            items: targets,
        })
    }
}

fn conversation_identity_ids(
    connection: &rusqlite::Connection,
    conversation_id: &str,
) -> Result<Vec<String>> {
    let canonical_id: String = connection.query_row(
        &format!(
            "{CHAT_IDENTITY_CTES} SELECT coalesce((SELECT canonical_conversation_id FROM conversation_aliases WHERE legacy_conversation_id=?1),?1)"
        ),
        [conversation_id],
        |row| row.get(0),
    )?;
    let mut statement = connection.prepare(&format!(
        "{CHAT_IDENTITY_CTES} SELECT canonical_conversation_id,legacy_conversation_id
         FROM conversation_aliases
         WHERE canonical_conversation_id=?1
         ORDER BY legacy_conversation_id"
    ))?;
    let aliases = statement
        .query_map([&canonical_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if aliases.is_empty() {
        return Ok(vec![canonical_id]);
    }
    let canonical = aliases[0].0.clone();
    let mut ids = Vec::with_capacity(aliases.len() + 1);
    ids.push(canonical.clone());
    ids.extend(
        aliases
            .into_iter()
            .map(|(_, legacy)| legacy)
            .filter(|id| id != &canonical),
    );
    ids.sort();
    ids.dedup();
    if let Some(position) = ids.iter().position(|id| id == &canonical) {
        ids.swap(0, position);
    }
    Ok(ids)
}

fn read_target_tags_for_ids(
    connection: &rusqlite::Connection,
    target_kind: &str,
    target_ids: &[String],
) -> Result<Vec<String>> {
    let json_ids = serde_json::to_string(target_ids)
        .map_err(|error| Error::Configuration(error.to_string()))?;
    let mut statement = connection.prepare(
        "SELECT t.name FROM (SELECT x.name_key,min(CAST(ids.key AS INTEGER)) AS target_order,min(x.ordinal) AS tag_order FROM json_each(?1) ids JOIN target_tags x ON x.target_id=ids.value AND x.target_kind=?2 GROUP BY x.name_key) picked JOIN tags t ON t.name_key=picked.name_key ORDER BY picked.target_order,picked.tag_order,picked.name_key",
    )?;
    let names = statement
        .query_map(params![json_ids, target_kind], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(names)
}

fn read_target_tags(
    connection: &rusqlite::Connection,
    target_kind: &str,
    target_id: &str,
) -> Result<TaggedTargetSnapshot> {
    let mut stmt = connection.prepare(
        "SELECT t.name FROM target_tags x JOIN tags t ON t.name_key=x.name_key WHERE x.target_kind=?1 AND x.target_id=?2 ORDER BY x.ordinal,t.name_key",
    )?;
    let tags = stmt
        .query_map(params![target_kind, target_id], |row| row.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()?;
    Ok(TaggedTargetSnapshot {
        target_kind: target_kind.to_owned(),
        target_id: target_id.to_owned(),
        tags,
        tag_revision: 0,
    })
}

fn read_target_tag_keys(
    connection: &rusqlite::Connection,
    target_kind: &str,
    target_id: &str,
) -> Result<Vec<String>> {
    let mut stmt = connection.prepare(
        "SELECT name_key FROM target_tags WHERE target_kind=?1 AND target_id=?2 ORDER BY ordinal,name_key",
    )?;
    let values = stmt
        .query_map(params![target_kind, target_id], |row| row.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()?;
    Ok(values)
}

fn current_tag_revision(connection: &rusqlite::Connection) -> Result<i64> {
    Ok(connection.query_row(
        "SELECT revision FROM tag_state WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_tags_union_all_verified_archive_aliases_and_replace_them_atomically() {
        let mut store = Store::in_memory().unwrap();
        let current_scope = format!("chatgpt:observed:{}", "a".repeat(64));
        let uuid = "12345678-1234-4abc-8def-1234567890ab";
        store
            .connection
            .execute(
                "INSERT INTO conversations(id,title,provider,account_scope,source_id) VALUES
             ('direct','Current','chatgpt',?1,?2),
             ('cache-a','Archive A','paseo-chatgpt','paseo-cache:owner-a',?2),
             ('cache-b','Archive B','paseo-chatgpt','paseo-cache:owner-b',?3)",
                params![current_scope, uuid, format!("conversation:{uuid}")],
            )
            .unwrap();
        for (key, name, id) in [
            ("current", "Current", "direct"),
            ("legacy-a", "Legacy A", "cache-a"),
            ("legacy-b", "Legacy B", "cache-b"),
        ] {
            store
                .connection
                .execute(
                    "INSERT INTO tags(name_key,name) VALUES(?1,?2)",
                    params![key, name],
                )
                .unwrap();
            store.connection.execute(
                "INSERT INTO target_tags(target_kind,target_id,name_key,ordinal) VALUES('conversation',?1,?2,0)",
                params![id, key],
            ).unwrap();
        }
        let from_first_alias = store.target_tags("conversation", "cache-a").unwrap();
        assert_eq!(from_first_alias, ["Current", "Legacy A", "Legacy B"]);
        let from_second_alias = store.target_tags("conversation", "cache-b").unwrap();
        assert_eq!(from_second_alias, from_first_alias);

        store
            .set_target_tags("conversation", "cache-a", &["Kept".to_owned()])
            .unwrap();
        for id in ["direct", "cache-a", "cache-b"] {
            assert_eq!(store.target_tags("conversation", id).unwrap(), ["Kept"]);
        }
        let page = store
            .tagged_targets_snapshot(Some("conversation"), "kept", 10, None)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].target_id, "direct");
    }

    #[test]
    fn tags_replace_dedupe_casefold_and_filter_by_target_kind() {
        let mut store = Store::in_memory().unwrap();
        store.connection.execute("INSERT INTO conversations(id,title,provider) VALUES('conversation:one','One','chatgpt')", []).unwrap();
        let first = store
            .set_target_tags(
                "conversation",
                "conversation:one",
                &["  Work   Notes ".into(), "work notes".into(), "MVP".into()],
            )
            .unwrap();
        assert_eq!(first.tags, ["Work Notes", "MVP"]);
        let first_revision = first.tag_revision;
        let unchanged = store
            .set_target_tags(
                "conversation",
                "conversation:one",
                &["Work Notes".into(), "MVP".into()],
            )
            .unwrap();
        assert_eq!(unchanged.tag_revision, first_revision);
        store
            .set_target_tags("entity", "entity:opaque-123", &["Work Notes".into()])
            .unwrap();
        let page = store
            .tagged_targets_snapshot(None, "work notes", 1, None)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].tag_revision, page.tag_revision);
        assert!(store
            .target_tags("conversation", "conversation:one")
            .unwrap()
            .contains(&"Work Notes".to_owned()));
        assert_eq!(
            store
                .target_tags("conversation", "conversation:one")
                .unwrap(),
            ["Work Notes", "MVP"]
        );
        assert_eq!(
            store.list_tags(None, Some("WORK"), 20).unwrap()[0].target_count,
            2
        );
        assert_eq!(
            store.list_tags(Some("entity"), None, 20).unwrap()[0].target_count,
            1
        );
        assert_eq!(
            store.tagged_targets(None, "work notes", 1, None).unwrap()[0].target_id,
            "conversation:one"
        );
        assert_eq!(
            store
                .tagged_targets(
                    None,
                    "work notes",
                    10,
                    Some(("conversation", "conversation:one"))
                )
                .unwrap()
                .len(),
            1
        );
        let cleared = store
            .set_target_tags("conversation", "conversation:one", &[])
            .unwrap();
        assert!(cleared.tags.is_empty());
        assert!(cleared.tag_revision > first_revision);
        assert_eq!(
            store.list_tags(None, None, 20).unwrap()[0].name,
            "Work Notes"
        );
        assert_eq!(store.list_tags(None, None, 20).unwrap()[0].target_count, 1);
    }

    #[test]
    fn target_tags_survive_catalog_table_rebuild_and_store_reopen() {
        let mut store = Store::in_memory().unwrap();
        store
            .connection
            .execute(
                "INSERT INTO conversations(id,title,provider) VALUES('c1','One','chatgpt')",
                [],
            )
            .unwrap();
        store
            .set_target_tags("file", "entity:file-digest", &["reference".into()])
            .unwrap();
        store
            .connection
            .execute_batch("CREATE TABLE catalog_chats(id TEXT); DROP TABLE catalog_chats;")
            .unwrap();
        assert_eq!(
            store.target_tags("file", "entity:file-digest").unwrap(),
            ["reference"]
        );
    }

    #[test]
    fn artifact_bytes_stay_private_from_json_export_but_metadata_is_exported() {
        let store = Store::in_memory().unwrap();
        store.connection.execute("INSERT INTO conversations(id,title,provider) VALUES('conversation:file','One','chatgpt')", []).unwrap();
        store.connection.execute(
            "INSERT INTO artifact_contents(entity_id,conversation_id,filename,mime_type,byte_size,sha256,content,saved_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params!["entity:file-digest", "conversation:file", "answer.pdf", "application/pdf", 13_i64, "a".repeat(64), b"private bytes".as_slice(), 1_i64],
        ).unwrap();
        let mut output = Vec::new();
        store.write_library_json(&mut output).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(document["artifact_contents"][0]["filename"], "answer.pdf");
        assert_eq!(document["artifact_contents"][0]["content_included"], false);
        assert!(document["artifact_contents"][0].get("content").is_none());
        assert!(!String::from_utf8(output).unwrap().contains("private bytes"));
    }

    #[test]
    fn invalid_tag_inputs_are_rejected_without_partial_replacement() {
        let mut store = Store::in_memory().unwrap();
        store
            .connection
            .execute(
                "INSERT INTO conversations(id,title,provider) VALUES('c1','One','chatgpt')",
                [],
            )
            .unwrap();
        store
            .set_target_tags("conversation", "c1", &["keep".into()])
            .unwrap();
        assert!(store
            .set_target_tags("invalid", "c1", &["new".into()])
            .is_err());
        assert!(store
            .set_target_tags("conversation", "c1", &["bad\nname".into()])
            .is_err());
        assert_eq!(store.target_tags("conversation", "c1").unwrap(), ["keep"]);
    }

    #[test]
    fn verified_upstream_title_commit_is_scope_bound_and_clears_local_alias() {
        let mut store = Store::in_memory().unwrap();
        let scope = format!("chatgpt:observed:{}", "a".repeat(64));
        let project_key = crate::store::project_key("chatgpt", &scope, "g-p-fixture");
        store.connection.execute(
            "INSERT INTO projects(project_key,provider,account_scope,source_id,title,local_name) VALUES(?1,'chatgpt',?2,'g-p-fixture','Old name',NULL)",
            params![project_key, scope],
        ).unwrap();
        store
            .rename_project(&project_key, Some("Local alias"))
            .unwrap();
        assert!(store
            .commit_verified_project_title(
                &project_key,
                &format!("chatgpt:observed:{}", "b".repeat(64)),
                "Renamed"
            )
            .is_err());
        let committed = store
            .commit_verified_project_title(&project_key, &scope, "Renamed")
            .unwrap();
        assert_eq!(committed.project["title"], "Renamed");
        assert_eq!(committed.project["display_name"], "Renamed");
        assert_eq!(committed.project["name_source"], "upstream");
        let listed = store.projects_snapshot("chatgpt").unwrap();
        assert_eq!(listed.projects[0]["display_name"], "Renamed");
    }
}
