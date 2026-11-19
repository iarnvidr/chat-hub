use super::Store;
use crate::{
    artifacts::{content_sha256, valid_filename, SavedArtifact, MAX_ARTIFACT_BYTES},
    catalog::CatalogEntityKind,
    error::{Error, Result},
};
use rusqlite::{params, OptionalExtension};

fn metadata(row: &rusqlite::Row<'_>) -> rusqlite::Result<SavedArtifact> {
    Ok(SavedArtifact {
        entity_id: row.get(0)?,
        conversation_id: row.get(1)?,
        filename: row.get(2)?,
        mime_type: row.get(3)?,
        byte_size: row.get(4)?,
        sha256: row.get(5)?,
        saved_at_ms: row.get(6)?,
    })
}

impl Store {
    /// A caller must explicitly select the bytes to associate with a known file.
    /// No host path or remote URL is accepted by this storage operation.
    pub fn save_artifact_content(
        &mut self,
        entity_id: &str,
        conversation_id: &str,
        filename: &str,
        mime_type: Option<&str>,
        bytes: &[u8],
    ) -> Result<SavedArtifact> {
        if bytes.len() > MAX_ARTIFACT_BYTES
            || !valid_filename(filename)
            || mime_type.is_some_and(|m| m.len() > 255 || m.chars().any(char::is_control))
        {
            return Err(Error::Configuration("invalid artifact content".into()));
        }
        let detail = self
            .catalog_entity(entity_id)?
            .ok_or_else(|| Error::Configuration("artifact not found".into()))?;
        let conversation_references_entity = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM catalog_entity_refs WHERE entity_id=?1 AND conversation_id=?2)",
            params![entity_id, conversation_id],
            |row| row.get::<_, i64>(0),
        )? != 0;
        if detail.entity.kind != CatalogEntityKind::FileArtifact || !conversation_references_entity
        {
            return Err(Error::Configuration(
                "artifact conversation mismatch".into(),
            ));
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::Configuration("clock unavailable".into()))?
            .as_millis() as i64;
        let saved = SavedArtifact {
            entity_id: entity_id.into(),
            conversation_id: conversation_id.into(),
            filename: filename.into(),
            mime_type: mime_type.map(str::to_owned),
            byte_size: bytes.len() as i64,
            sha256: content_sha256(bytes),
            saved_at_ms: stamp,
        };
        self.connection.execute(
            "INSERT INTO artifact_contents(entity_id,conversation_id,filename,mime_type,byte_size,sha256,content,saved_at_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(entity_id) DO UPDATE SET conversation_id=excluded.conversation_id,
             filename=excluded.filename,mime_type=excluded.mime_type,byte_size=excluded.byte_size,
             sha256=excluded.sha256,content=excluded.content,saved_at_ms=excluded.saved_at_ms",
            params![saved.entity_id, saved.conversation_id, saved.filename, saved.mime_type,
                saved.byte_size, saved.sha256, bytes, saved.saved_at_ms],
        )?;
        Ok(saved)
    }

    pub fn saved_artifact(&self, entity_id: &str) -> Result<Option<SavedArtifact>> {
        Ok(self
            .connection
            .query_row(
                "SELECT entity_id,conversation_id,filename,mime_type,byte_size,sha256,saved_at_ms
             FROM artifact_contents WHERE entity_id=?1",
                [entity_id],
                metadata,
            )
            .optional()?)
    }

    pub fn saved_artifacts(
        &self,
        conversation_id: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SavedArtifact>> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Configuration("invalid file limit".into()));
        }
        let mut statement = self.connection.prepare(
            "SELECT entity_id,conversation_id,filename,mime_type,byte_size,sha256,saved_at_ms
             FROM artifact_contents WHERE (?1 IS NULL OR conversation_id=?1)
             AND (?2 IS NULL OR entity_id>?2) ORDER BY entity_id LIMIT ?3",
        )?;
        let rows = statement.query_map(params![conversation_id, after, limit as i64], metadata)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn artifact_content(&self, entity_id: &str) -> Result<Option<(SavedArtifact, Vec<u8>)>> {
        let Some(saved) = self.saved_artifact(entity_id)? else {
            return Ok(None);
        };
        if saved.byte_size < 0 || saved.byte_size as usize > MAX_ARTIFACT_BYTES {
            return Err(Error::Configuration("invalid artifact size".into()));
        }
        let bytes: Vec<u8> = self.connection.query_row(
            "SELECT content FROM artifact_contents WHERE entity_id=?1",
            [entity_id],
            |r| r.get(0),
        )?;
        if bytes.len() != saved.byte_size as usize || content_sha256(&bytes) != saved.sha256 {
            return Err(Error::Configuration(
                "artifact integrity check failed".into(),
            ));
        }
        Ok(Some((saved, bytes)))
    }
}
