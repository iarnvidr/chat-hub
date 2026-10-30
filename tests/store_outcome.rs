use chat_hub::store::{IngestOutcome, Observation, Store};
use serde_json::json;
fn observation(title: &str, update: f64) -> Observation {
    Observation {
        provider: "chatgpt".into(),
        account_scope: "provisional:default".into(),
        source_id: "outcome".into(),
        title: title.into(),
        url: None,
        current_node: None,
        graph: vec![],
        raw_payload: json!({"update_time":update}),
        source_time: None,
        observed_ms: 1,
        parser: "test".into(),
        completeness: "complete".into(),
    }
}
#[test]
fn outcome_tracks_import_update_and_stale_unchanged() {
    let mut s = Store::in_memory().unwrap();
    let id = match s
        .ingest_observation_outcome(&observation("one", 1.))
        .unwrap()
    {
        IngestOutcome::Imported { conversation_id } => conversation_id,
        x => panic!("{x:?}"),
    };
    assert_eq!(
        s.ingest_observation_outcome(&observation("one", 1.))
            .unwrap(),
        IngestOutcome::Unchanged {
            conversation_id: id.clone()
        }
    );
    assert_eq!(
        s.ingest_observation_outcome(&observation("two", 2.))
            .unwrap(),
        IngestOutcome::Updated {
            conversation_id: id.clone()
        }
    );
    assert_eq!(
        s.ingest_observation_outcome(&observation("stale", 1.))
            .unwrap(),
        IngestOutcome::Unchanged {
            conversation_id: id
        }
    );
}
#[test]
fn failed_ingest_does_not_partially_commit() {
    let mut s = Store::in_memory().unwrap();
    let mut o = observation("bad", 1.);
    o.graph.push(chat_hub::store::GraphNode {
        source_node_id: "x".into(),
        source_message_id: None,
        parent_id: None,
        role: None,
        author: None,
        create_time: None,
        parts: vec![],
    });
    o.graph.push(o.graph[0].clone());
    assert!(s.ingest_observation_outcome(&o).is_err());
    assert_eq!(s.library_revision().unwrap(), 0);
    assert!(s
        .read_normalized("conversation:[\"chatgpt\",\"provisional:default\",\"outcome\"]")
        .unwrap()
        .is_none());
}
