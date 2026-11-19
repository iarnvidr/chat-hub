use super::{normalized_upstream_conversation_id, Store};
use crate::error::{Error, Result};
use rusqlite::{params, OptionalExtension};
use std::collections::HashSet;

const OBSERVED_SCOPE_PREFIX: &str = "chatgpt:observed:";

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedConversationStateReceipt {
    pub verified: bool,
    pub conversation_id: String,
    pub source_conversation_id: String,
    pub scope: String,
    pub target_provider: String,
    pub target_scope: String,
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedChatgptPinSnapshotReceipt {
    pub verified: bool,
    pub complete: bool,
    pub scope: String,
    pub pinned_source_ids: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ChatgptPinSnapshotSummary {
    pub verified: bool,
    pub pinned_count: usize,
    pub matched_count: usize,
    pub unknown_count: usize,
    pub library_revision: i64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ConversationStateChange {
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ConversationStateSnapshot {
    pub conversation_id: String,
    pub pinned: bool,
    pub archived: bool,
    pub library_revision: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ConversationStateIdentity {
    pub conversation_id: String,
    pub provider: String,
    pub account_scope: String,
    pub source_id: String,
    pub pinned: bool,
    pub archived: bool,
    pub library_revision: i64,
}

impl Store {
    /// Return only the local row identity and action state. This avoids loading
    /// transcript content when a native provider action needs a binding.
    pub fn conversation_state_identity(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationStateIdentity>> {
        if conversation_id.trim().is_empty()
            || conversation_id.len() > 1024
            || conversation_id.chars().any(char::is_control)
        {
            return Err(Error::Configuration("invalid conversation id".into()));
        }
        let value = self
            .connection
            .query_row(
                "SELECT c.id,c.provider,c.account_scope,c.source_id,
                    coalesce(cs.pinned,0),coalesce(cs.archived,0),ls.revision
             FROM conversations c
             LEFT JOIN conversation_local_state cs ON cs.conversation_id=c.id
             JOIN library_state ls ON ls.singleton=1
             WHERE c.id=?1",
                [conversation_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)? != 0,
                        row.get::<_, i64>(5)? != 0,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            conversation_id,
            provider,
            account_scope,
            source_id,
            pinned,
            archived,
            library_revision,
        )) = value
        else {
            return Ok(None);
        };
        if provider.len() > 128
            || account_scope.len() > 1024
            || source_id.len() > 1024
            || provider.chars().any(char::is_control)
            || account_scope.chars().any(char::is_control)
            || source_id.chars().any(char::is_control)
        {
            return Err(Error::Configuration(
                "invalid stored conversation identity".into(),
            ));
        }
        Ok(Some(ConversationStateIdentity {
            conversation_id,
            provider,
            account_scope,
            source_id,
            pinned,
            archived,
            library_revision,
        }))
    }

    /// Persist only state that was read back from the provider. The receipt also
    /// binds the observed provider resource to one existing local row.
    pub fn commit_verified_conversation_state(
        &mut self,
        conversation_id: &str,
        receipt: &VerifiedConversationStateReceipt,
        change: ConversationStateChange,
    ) -> Result<ConversationStateSnapshot> {
        if conversation_id.trim().is_empty()
            || conversation_id.len() > 1024
            || receipt.conversation_id != conversation_id
            || !receipt.verified
            || !valid_observed_scope(&receipt.scope)
            || !matches!(
                receipt.target_provider.as_str(),
                "chatgpt" | "paseo-chatgpt"
            )
            || receipt.target_scope.is_empty()
            || receipt.target_scope.len() > 256
            || receipt.target_scope.chars().any(char::is_control)
            || receipt.source_conversation_id.is_empty()
            || receipt.source_conversation_id.len() > 1024
            || receipt.source_conversation_id.chars().any(char::is_control)
            || change.archived.is_none() && change.pinned.is_none()
            || change
                .archived
                .is_some_and(|value| receipt.archived != Some(value))
            || change
                .pinned
                .is_some_and(|value| receipt.pinned != Some(value))
        {
            return Err(Error::Configuration(
                "conversation state requires a matching verified provider receipt".into(),
            ));
        }

        let tx = self.connection.transaction()?;
        let target: Option<(String, String, String)> = tx
            .query_row(
                "SELECT provider,account_scope,source_id FROM conversations WHERE id=?1",
                [conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((provider, account_scope, source_id)) = target else {
            return Err(Error::Configuration("conversation not found".into()));
        };
        if provider != receipt.target_provider
            || account_scope != receipt.target_scope
            || source_id != receipt.source_conversation_id
        {
            return Err(Error::Configuration(
                "verified receipt does not match the local conversation".into(),
            ));
        }

        let prior = tx
            .query_row(
                "SELECT pinned,archived,pinned_scope FROM conversation_local_state WHERE conversation_id=?1",
                [conversation_id],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, i64>(1)? != 0, row.get::<_, Option<String>>(2)?)),
            )
            .optional()?;
        let (mut pinned, mut archived, prior_pin_scope) =
            prior.clone().unwrap_or((false, false, None));
        if let Some(value) = change.pinned {
            pinned = value;
        }
        if let Some(value) = change.archived {
            archived = value;
        }
        let pinned_scope = if change.pinned.is_some() {
            Some(receipt.scope.clone())
        } else {
            prior_pin_scope
        };
        if prior != Some((pinned, archived, pinned_scope.clone())) {
            tx.execute(
                "INSERT INTO conversation_local_state(conversation_id,pinned,archived,pinned_scope,updated_at_ms) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(conversation_id) DO UPDATE SET pinned=excluded.pinned,archived=excluded.archived,pinned_scope=excluded.pinned_scope,updated_at_ms=excluded.updated_at_ms",
                params![conversation_id, pinned, archived, pinned_scope, now_ms()],
            )?;
            tx.execute(
                "UPDATE library_state SET revision=revision+1 WHERE singleton=1",
                [],
            )?;
        }
        let revision = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        tx.commit()?;
        Ok(ConversationStateSnapshot {
            conversation_id: conversation_id.to_owned(),
            pinned,
            archived,
            library_revision: revision,
        })
    }

    /// Reconcile a complete, account-verified pin listing against local rows.
    /// Direct ChatGPT rows are scoped by their stored account. Legacy cache rows
    /// are associated only on a unique upstream source ID match and are cleared
    /// only when a prior verified pin write tied them to this same scope.
    pub fn apply_verified_chatgpt_pin_snapshot(
        &mut self,
        receipt: &VerifiedChatgptPinSnapshotReceipt,
    ) -> Result<ChatgptPinSnapshotSummary> {
        const MAX_PINNED_IDS: usize = 1_000;
        if !receipt.verified
            || !receipt.complete
            || !valid_observed_scope(&receipt.scope)
            || receipt.pinned_source_ids.len() > MAX_PINNED_IDS
        {
            return Err(Error::Configuration(
                "incomplete or unverified ChatGPT pin snapshot".into(),
            ));
        }
        let mut unique_ids = HashSet::with_capacity(receipt.pinned_source_ids.len());
        let mut pinned_ids = Vec::with_capacity(receipt.pinned_source_ids.len());
        for source_id in &receipt.pinned_source_ids {
            let Some(normalized) = normalized_upstream_conversation_id(source_id) else {
                return Err(Error::Configuration(
                    "invalid conversation ID in pin snapshot".into(),
                ));
            };
            let normalized = normalized.to_ascii_lowercase();
            if !unique_ids.insert(normalized.clone()) {
                return Err(Error::Configuration(
                    "duplicate conversation ID in pin snapshot".into(),
                ));
            }
            pinned_ids.push(normalized);
        }
        let ids_json = serde_json::to_string(&pinned_ids)
            .map_err(|error| Error::Configuration(error.to_string()))?;
        let tx = self.connection.transaction()?;
        let now = now_ms();
        let direct_id = "lower(CASE WHEN substr(c.source_id,1,13)='conversation:' THEN substr(c.source_id,14) ELSE c.source_id END)";
        let direct_match = format!(
            "EXISTS(SELECT 1 FROM conversations c JOIN json_each(?2) ids ON ids.value={direct_id} WHERE c.id=conversation_local_state.conversation_id)"
        );
        let direct_scope = "conversation_id IN (SELECT id FROM conversations WHERE provider='chatgpt' AND account_scope=?1)";
        let mut changed = tx.execute(
            &format!(
                "UPDATE conversation_local_state SET pinned={direct_match},pinned_scope=?1,updated_at_ms=?3 WHERE {direct_scope} AND (pinned<>{direct_match} OR pinned_scope IS NOT ?1)"
            ),
            params![receipt.scope, ids_json, now],
        )?;
        let insert_direct_sql = format!(
            "INSERT INTO conversation_local_state(conversation_id,pinned,archived,pinned_scope,updated_at_ms) SELECT c.id,EXISTS(SELECT 1 FROM json_each(?2) ids WHERE ids.value={direct_id}),0,?1,?3 FROM conversations c WHERE c.provider='chatgpt' AND c.account_scope=?1 AND NOT EXISTS(SELECT 1 FROM conversation_local_state s WHERE s.conversation_id=c.id)"
        );
        changed += tx.execute(&insert_direct_sql, params![receipt.scope, ids_json, now])?;

        let direct_matched: usize = tx.query_row(
            &format!(
                "SELECT count(DISTINCT c.id) FROM conversations c JOIN json_each(?1) ids ON ids.value={direct_id} WHERE c.provider='chatgpt' AND c.account_scope=?2"
            ),
            params![ids_json, receipt.scope],
            |row| row.get(0),
        )?;

        let cache_id = "lower(CASE WHEN substr(c.source_id,1,13)='conversation:' THEN substr(c.source_id,14) ELSE c.source_id END)";
        let has_direct_sql = format!(
            "SELECT EXISTS(SELECT 1 FROM conversations c WHERE c.provider='chatgpt' AND c.account_scope=?2 AND {direct_id}=?1)"
        );
        let cached_rows_sql = format!(
            "SELECT c.id,s.pinned_scope FROM conversations c LEFT JOIN conversation_local_state s ON s.conversation_id=c.id WHERE c.provider='paseo-chatgpt' AND c.account_scope LIKE 'paseo-cache:%' AND {cache_id}=?1 LIMIT 2"
        );
        let upsert_cached_sql = "INSERT INTO conversation_local_state(conversation_id,pinned,archived,pinned_scope,updated_at_ms) VALUES(?1,1,coalesce((SELECT archived FROM conversation_local_state WHERE conversation_id=?1),0),?2,?3) ON CONFLICT(conversation_id) DO UPDATE SET pinned=1,pinned_scope=excluded.pinned_scope,updated_at_ms=excluded.updated_at_ms WHERE pinned<>1 OR pinned_scope IS NOT excluded.pinned_scope";
        let mut matched_count = direct_matched;
        let mut unknown_count = 0_usize;
        // Keep unchanged, uniquely matched legacy pins in place. Clear only rows
        // absent from this complete snapshot or made ambiguous by duplicate IDs.
        changed += tx.execute(
            &format!(
                "UPDATE conversation_local_state SET pinned=0,updated_at_ms=?3 WHERE pinned=1 AND pinned_scope=?1 AND conversation_id IN (SELECT c.id FROM conversations c WHERE c.provider='paseo-chatgpt' AND c.account_scope LIKE 'paseo-cache:%') AND NOT EXISTS(SELECT 1 FROM conversations c JOIN json_each(?2) ids ON ids.value={cache_id} WHERE c.id=conversation_local_state.conversation_id AND (SELECT count(*) FROM conversations matched WHERE matched.provider='paseo-chatgpt' AND matched.account_scope LIKE 'paseo-cache:%' AND lower(CASE WHEN substr(matched.source_id,1,13)='conversation:' THEN substr(matched.source_id,14) ELSE matched.source_id END)=ids.value)=1)"
            ),
            params![receipt.scope, ids_json, now],
        )?;
        {
            let mut direct = tx.prepare(&has_direct_sql)?;
            let mut cached = tx.prepare(&cached_rows_sql)?;
            for source_id in &pinned_ids {
                if direct.query_row(params![source_id, receipt.scope], |row| {
                    row.get::<_, bool>(0)
                })? {
                    continue;
                }
                let rows = cached
                    .query_map([source_id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                if let [(conversation_id, prior_scope)] = rows.as_slice() {
                    if prior_scope
                        .as_deref()
                        .is_some_and(|scope| scope != receipt.scope)
                    {
                        unknown_count += 1;
                        continue;
                    }
                    changed += tx.execute(
                        upsert_cached_sql,
                        params![conversation_id, receipt.scope, now],
                    )?;
                    matched_count += 1;
                } else {
                    unknown_count += 1;
                }
            }
        }
        if changed > 0 {
            tx.execute(
                "UPDATE library_state SET revision=revision+1 WHERE singleton=1",
                [],
            )?;
        }
        let library_revision = tx.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        tx.commit()?;
        Ok(ChatgptPinSnapshotSummary {
            verified: true,
            pinned_count: pinned_ids.len(),
            matched_count,
            unknown_count,
            library_revision,
        })
    }
}

fn valid_observed_scope(scope: &str) -> bool {
    scope
        .strip_prefix(OBSERVED_SCOPE_PREFIX)
        .is_some_and(|digest| digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(super) fn state_flag(
    conn: &rusqlite::Connection,
    conversation_id: &str,
    field: &str,
) -> Result<bool> {
    let column = match field {
        "pinned" => "pinned",
        "archived" => "archived",
        _ => return Err(Error::Configuration("invalid state field".into())),
    };
    let sql = format!(
        "SELECT coalesce((SELECT {column} FROM conversation_local_state WHERE conversation_id=?1),0)"
    );
    Ok(conn.query_row(&sql, [conversation_id], |row| row.get::<_, i64>(0))? != 0)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::VerifiedChatgptPinSnapshotReceipt;
    use crate::store::Store;

    const SCOPE_A: &str =
        "chatgpt:observed:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SCOPE_B: &str =
        "chatgpt:observed:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const ID_A: &str = "123e4567-e89b-12d3-a456-426614174000";
    const ID_B: &str = "123e4567-e89b-12d3-a456-426614174001";
    const ID_C: &str = "123e4567-e89b-12d3-a456-426614174002";
    const ID_D: &str = "123e4567-e89b-12d3-a456-426614174003";

    fn seeded_store() -> Store {
        let store = Store::in_memory().unwrap();
        for (id, provider, scope, source_id, pinned_scope) in [
            ("direct-a", "chatgpt", SCOPE_A, ID_A, Some(SCOPE_A)),
            ("direct-b", "chatgpt", SCOPE_A, ID_B, Some(SCOPE_A)),
            ("other-account", "chatgpt", SCOPE_B, ID_A, Some(SCOPE_B)),
            (
                "legacy-c",
                "paseo-chatgpt",
                "paseo-cache:legacy",
                &format!("conversation:{ID_C}"),
                Some(SCOPE_A),
            ),
            (
                "legacy-d",
                "paseo-chatgpt",
                "paseo-cache:legacy",
                ID_D,
                Some(SCOPE_B),
            ),
        ] {
            store.connection.execute(
                "INSERT INTO conversations(id,title,provider,account_scope,source_id) VALUES(?1,'test',?2,?3,?4)",
                rusqlite::params![id, provider, scope, source_id],
            ).unwrap();
            store.connection.execute(
                "INSERT INTO conversation_local_state(conversation_id,pinned,archived,pinned_scope,updated_at_ms) VALUES(?1,1,0,?2,1)",
                rusqlite::params![id, pinned_scope],
            ).unwrap();
        }
        store
    }

    fn receipt(scope: &str, complete: bool, ids: &[&str]) -> VerifiedChatgptPinSnapshotReceipt {
        VerifiedChatgptPinSnapshotReceipt {
            verified: true,
            complete,
            scope: scope.to_owned(),
            pinned_source_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    fn pinned(store: &Store, id: &str) -> bool {
        store
            .connection
            .query_row(
                "SELECT pinned FROM conversation_local_state WHERE conversation_id=?1",
                [id],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    }

    #[test]
    fn complete_snapshot_reconciles_current_scope_and_legacy_ids_only() {
        let mut store = seeded_store();
        let result = store
            .apply_verified_chatgpt_pin_snapshot(&receipt(SCOPE_A, true, &[ID_A, ID_C]))
            .unwrap();
        assert!(result.verified);
        assert_eq!(result.pinned_count, 2);
        assert_eq!(result.matched_count, 2);
        assert_eq!(result.unknown_count, 0);
        assert_eq!(result.library_revision, 1);
        assert!(pinned(&store, "direct-a"));
        assert!(!pinned(&store, "direct-b"));
        assert!(pinned(&store, "other-account"));
        assert!(pinned(&store, "legacy-c"));
        assert!(pinned(&store, "legacy-d"));

        let repeated = store
            .apply_verified_chatgpt_pin_snapshot(&receipt(SCOPE_A, true, &[ID_A, ID_C]))
            .unwrap();
        assert_eq!(repeated.library_revision, result.library_revision);

        store
            .apply_verified_chatgpt_pin_snapshot(&receipt(SCOPE_A, true, &[]))
            .unwrap();
        assert!(!pinned(&store, "direct-a"));
        assert!(!pinned(&store, "direct-b"));
        assert!(pinned(&store, "other-account"));
        assert!(!pinned(&store, "legacy-c"));
        assert!(pinned(&store, "legacy-d"));
    }

    #[test]
    fn incomplete_or_ambiguous_snapshot_is_rejected_without_state_changes() {
        let mut store = seeded_store();
        let before: i64 = store
            .connection
            .query_row(
                "SELECT revision FROM library_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        for invalid in [
            receipt(SCOPE_A, false, &[]),
            receipt(SCOPE_A, true, &[ID_A, ID_A]),
            receipt(SCOPE_A, true, &["not-an-id"]),
        ] {
            assert!(store.apply_verified_chatgpt_pin_snapshot(&invalid).is_err());
        }
        let after: i64 = store
            .connection
            .query_row(
                "SELECT revision FROM library_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
        for id in [
            "direct-a",
            "direct-b",
            "other-account",
            "legacy-c",
            "legacy-d",
        ] {
            assert!(pinned(&store, id));
        }
    }

    #[test]
    fn unknown_pinned_ids_are_reported_without_matching_by_title_or_scope_guess() {
        let mut store = seeded_store();
        let result = store
            .apply_verified_chatgpt_pin_snapshot(&receipt(
                SCOPE_A,
                true,
                &["123e4567-e89b-12d3-a456-426614174099"],
            ))
            .unwrap();
        assert_eq!(result.pinned_count, 1);
        assert_eq!(result.matched_count, 0);
        assert_eq!(result.unknown_count, 1);
        assert!(!pinned(&store, "direct-a"));
        assert!(!pinned(&store, "direct-b"));
    }

    #[test]
    fn duplicate_legacy_source_id_clears_stale_current_scope_markers() {
        let mut store = seeded_store();
        store.connection.execute(
            "INSERT INTO conversations(id,title,provider,account_scope,source_id) VALUES('legacy-c-duplicate','duplicate','paseo-chatgpt','paseo-cache:duplicate',?1)",
            [format!("conversation:{ID_C}")],
        ).unwrap();
        store.connection.execute(
            "INSERT INTO conversation_local_state(conversation_id,pinned,archived,pinned_scope,updated_at_ms) VALUES('legacy-c-duplicate',1,0,?1,1)",
            [SCOPE_A],
        ).unwrap();

        let result = store
            .apply_verified_chatgpt_pin_snapshot(&receipt(SCOPE_A, true, &[ID_C]))
            .unwrap();
        assert_eq!(result.matched_count, 0);
        assert_eq!(result.unknown_count, 1);
        assert!(!pinned(&store, "legacy-c"));
        assert!(!pinned(&store, "legacy-c-duplicate"));
    }
}
