//! Persistence of sanitized operation shapes. Capture owns parsing and queues;
//! this adapter receives only the dependency-light observation contract.
use chat_hub_model::OperationObservation;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

impl super::Store {
    pub fn record_project_operation(
        &mut self,
        observation: &OperationObservation,
    ) -> rusqlite::Result<()> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS learned_project_operations(\
                id INTEGER PRIMARY KEY,\
                method TEXT NOT NULL, route TEXT NOT NULL, status INTEGER NOT NULL,\
                request_fields TEXT NOT NULL, response_fields TEXT NOT NULL,\
                first_seen_ms INTEGER NOT NULL, last_seen_ms INTEGER NOT NULL,\
                seen_count INTEGER NOT NULL DEFAULT 1,\
                UNIQUE(method,route,status,request_fields,response_fields)\
            );",
        )?;
        self.connection.execute(
            "INSERT INTO learned_project_operations(method,route,status,request_fields,response_fields,first_seen_ms,last_seen_ms,seen_count)\
             VALUES(?1,?2,?3,?4,?5,?6,?6,1)\
             ON CONFLICT(method,route,status,request_fields,response_fields)\
             DO UPDATE SET last_seen_ms=excluded.last_seen_ms,seen_count=min(learned_project_operations.seen_count+1,9223372036854775807)",
            params![observation.method, observation.route, observation.status, observation.request_fields, observation.response_fields, observation.observed_at_ms],
        )?;
        self.connection.execute(
            "DELETE FROM learned_project_operations WHERE id NOT IN (SELECT id FROM learned_project_operations ORDER BY last_seen_ms DESC,id DESC LIMIT 100)",
            [],
        )?;
        Ok(())
    }

    pub fn learned_project_operations(&self) -> rusqlite::Result<Value> {
        let exists: Option<i64> = self.connection.query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='learned_project_operations'",
            [], |row| row.get(0),
        ).optional()?;
        if exists.is_none() {
            return Ok(serde_json::json!({"operations":[]}));
        }
        let mut statement = self.connection.prepare(
            "SELECT method,route,status,request_fields,response_fields,first_seen_ms,last_seen_ms,seen_count \
             FROM learned_project_operations ORDER BY last_seen_ms DESC,method,route LIMIT 100",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(serde_json::json!({
                "method":row.get::<_,String>(0)?,"route":row.get::<_,String>(1)?,"status":row.get::<_,u16>(2)?,
                "request_fields":serde_json::from_str::<Value>(&row.get::<_,String>(3)?).unwrap_or(Value::Null),
                "response_fields":serde_json::from_str::<Value>(&row.get::<_,String>(4)?).unwrap_or(Value::Null),
                "first_seen_ms":row.get::<_,i64>(5)?,"last_seen_ms":row.get::<_,i64>(6)?,"seen_count":row.get::<_,u64>(7)?,
            }))
        })?.collect::<Result<Vec<_>,_>>()?;
        Ok(serde_json::json!({"operations":rows}))
    }
}
