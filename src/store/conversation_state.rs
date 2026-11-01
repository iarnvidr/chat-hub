use super::Store;
use crate::error::{Error, Result};
use rusqlite::{params, OptionalExtension};

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
                "SELECT pinned,archived FROM conversation_local_state WHERE conversation_id=?1",
                [conversation_id],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, i64>(1)? != 0)),
            )
            .optional()?;
        let (mut pinned, mut archived) = prior.unwrap_or((false, false));
        if let Some(value) = change.pinned {
            pinned = value;
        }
        if let Some(value) = change.archived {
            archived = value;
        }
        if prior != Some((pinned, archived)) {
            tx.execute(
                "INSERT INTO conversation_local_state(conversation_id,pinned,archived,updated_at_ms) VALUES(?1,?2,?3,?4) ON CONFLICT(conversation_id) DO UPDATE SET pinned=excluded.pinned,archived=excluded.archived,updated_at_ms=excluded.updated_at_ms",
                params![conversation_id, pinned, archived, now_ms()],
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
