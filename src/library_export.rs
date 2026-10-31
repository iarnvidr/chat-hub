use crate::{archive::AtomicOutput, error::Result, store::Store};
use rusqlite::Connection;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportMetadata {
    pub schema_version: i64,
    pub library_revision: i64,
    pub conversation_count: u64,
    pub node_count: u64,
    pub message_count: u64,
    pub observation_count: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReceipt {
    pub destination: PathBuf,
    pub metadata: ExportMetadata,
}

fn metadata(conn: &Connection) -> Result<ExportMetadata> {
    Ok(ExportMetadata {
        schema_version: conn.query_row("SELECT max(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })?,
        library_revision: conn.query_row(
            "SELECT revision FROM library_state WHERE singleton=1",
            [],
            |r| r.get(0),
        )?,
        conversation_count: conn.query_row("SELECT count(*) FROM conversations", [], |r| {
            r.get::<_, i64>(0)
        })? as u64,
        node_count: conn.query_row("SELECT count(*) FROM graph_nodes", [], |r| {
            r.get::<_, i64>(0)
        })? as u64,
        message_count: conn
            .query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))?
            as u64,
        observation_count: conn.query_row("SELECT count(*) FROM observations", [], |r| {
            r.get::<_, i64>(0)
        })? as u64,
    })
}

fn write_rows<W: Write>(
    conn: &Connection,
    w: &mut W,
    table: &str,
    sql: &str,
    columns: &[&str],
) -> Result<()> {
    write!(w, "\"{table}\":[")?;
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut first = true;
    while let Some(row) = rows.next()? {
        if !first {
            w.write_all(b",")?;
        }
        first = false;
        w.write_all(b"{")?;
        for (i, column) in columns.iter().enumerate() {
            if i > 0 {
                w.write_all(b",")?;
            }
            serde_json::to_writer(&mut *w, column)
                .map_err(|e| crate::error::Error::Configuration(e.to_string()))?;
            w.write_all(b":")?;
            let value: rusqlite::types::Value = row.get(i)?;
            match value {
                rusqlite::types::Value::Null => w.write_all(b"null")?,
                rusqlite::types::Value::Integer(v) => write!(w, "{v}")?,
                rusqlite::types::Value::Real(v) => serde_json::to_writer(&mut *w, &v)
                    .map_err(|e| crate::error::Error::Configuration(e.to_string()))?,
                rusqlite::types::Value::Text(v) => serde_json::to_writer(&mut *w, &v)
                    .map_err(|e| crate::error::Error::Configuration(e.to_string()))?,
                rusqlite::types::Value::Blob(v) => serde_json::to_writer(&mut *w, &v)
                    .map_err(|e| crate::error::Error::Configuration(e.to_string()))?,
            }
        }
        w.write_all(b"}")?;
    }
    w.write_all(b"]")?;
    Ok(())
}

fn write_artifact_metadata<W: Write>(conn: &Connection, w: &mut W) -> Result<()> {
    w.write_all(b"\"artifact_contents\":[")?;
    let mut stmt = conn.prepare("SELECT entity_id,conversation_id,filename,mime_type,byte_size,sha256,saved_at_ms FROM artifact_contents ORDER BY entity_id")?;
    let mut rows = stmt.query([])?;
    let mut first = true;
    while let Some(row) = rows.next()? {
        if !first {
            w.write_all(b",")?;
        }
        first = false;
        let value = serde_json::json!({
            "entity_id": row.get::<_, String>(0)?,
            "conversation_id": row.get::<_, String>(1)?,
            "filename": row.get::<_, String>(2)?,
            "mime_type": row.get::<_, Option<String>>(3)?,
            "byte_size": row.get::<_, i64>(4)?,
            "sha256": row.get::<_, String>(5)?,
            "saved_at_ms": row.get::<_, i64>(6)?,
            "content_included": false,
        });
        serde_json::to_writer(&mut *w, &value)
            .map_err(|e| crate::error::Error::Configuration(e.to_string()))?;
    }
    w.write_all(b"]")?;
    Ok(())
}
impl Store {
    pub fn write_library_json<W: Write>(&self, writer: &mut W) -> Result<ExportMetadata> {
        let tx = self.connection().unchecked_transaction()?;
        let meta = metadata(&tx)?;
        writer.write_all(b"{\"format\":\"chat-hub-library\",\"format_version\":1")?;
        write!(
            writer,
            ",\"schema_version\":{},\"library_revision\":{}",
            meta.schema_version, meta.library_revision
        )?;
        writer.write_all(b",")?;
        write_rows(&tx, writer, "conversations", "SELECT id,title,provider,url,updated_at,account_scope,source_id,current_node,last_source_updated_ms,ordering_payload,ordering_observed_ms,updated_at_ms FROM conversations ORDER BY id", &["id","title","provider","url","updated_at","account_scope","source_id","current_node","last_source_updated_ms","ordering_payload","ordering_observed_ms","updated_at_ms"])?;
        writer.write_all(b",")?;
        write_rows(&tx, writer, "nodes", "SELECT conversation_id,source_node_id,source_message_id,message_id,parent_id,role,author_json,create_time_json,parts_json,content FROM graph_nodes ORDER BY conversation_id,source_node_id", &["conversation_id","source_node_id","source_message_id","message_id","parent_id","role","author_json","create_time_json","parts_json","content"])?;
        writer.write_all(b",")?;
        write_rows(
            &tx,
            writer,
            "messages",
            "SELECT id,conversation_id,parent_id,role,content FROM messages ORDER BY id",
            &["id", "conversation_id", "parent_id", "role", "content"],
        )?;
        writer.write_all(b",")?;
        write_rows(&tx, writer, "observations", "SELECT id,conversation_id,source_time,observed_ms,parser,completeness,raw_payload FROM observations ORDER BY id", &["id","conversation_id","source_time","observed_ms","parser","completeness","raw_payload"])?;
        writer.write_all(b",")?;
        write_rows(
            &tx,
            writer,
            "tags",
            "SELECT name_key,name FROM tags ORDER BY name_key",
            &["name_key", "name"],
        )?;
        writer.write_all(b",")?;
        write_rows(&tx, writer, "target_tags", "SELECT target_kind,target_id,name_key,ordinal FROM target_tags ORDER BY target_kind,target_id,ordinal", &["target_kind","target_id","name_key","ordinal"])?;
        writer.write_all(b",")?;
        write_artifact_metadata(&tx, writer)?;
        writer.write_all(b"}\n")?;
        tx.commit()?;
        Ok(meta)
    }
    pub fn export_json_to(&self, destination: impl AsRef<Path>) -> Result<ExportReceipt> {
        let destination = destination.as_ref();
        let mut output = AtomicOutput::new(destination)?;
        let metadata = self.write_library_json(output.file())?;
        output.finish()?;
        Ok(ExportReceipt {
            destination: destination.to_path_buf(),
            metadata,
        })
    }
}
