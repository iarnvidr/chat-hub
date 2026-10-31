use chat_hub::{importer, store::Store};
use serde_json::json;

#[test]
fn paseo_owners_stay_separate_and_ordinary_files_cannot_claim_their_identity() {
    let mut store = Store::in_memory().unwrap();
    let document = json!({"id":"shared-source","title":"History conversation","mapping":{}});
    for owner in ["aaaaaaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbbbbbb"] {
        let bytes = serde_json::to_vec(&json!({"owner":owner,"conversations":[document]})).unwrap();
        assert_eq!(
            importer::import_paseo_history(&mut store, &bytes)
                .unwrap()
                .imported,
            1
        );
        assert_eq!(
            importer::import_paseo_history(&mut store, &bytes)
                .unwrap()
                .unchanged,
            1
        );
    }
    let ordinary =
        serde_json::to_vec(&json!({"owner":"aaaaaaaaaaaaaaaaaaaaaaaa","conversations":[document]}))
            .unwrap();
    assert_eq!(
        importer::import_chatgpt(&mut store, &ordinary)
            .unwrap()
            .imported,
        1
    );
    let rows = store.library_snapshot().unwrap().conversations;
    let mut scopes: Vec<_> = rows
        .iter()
        .map(|row| row["account_scope"].as_str().unwrap())
        .collect();
    scopes.sort_unstable();
    assert_eq!(
        scopes,
        [
            "paseo:aaaaaaaaaaaaaaaaaaaaaaaa",
            "paseo:bbbbbbbbbbbbbbbbbbbbbbbb",
            "provisional:default"
        ]
    );
}

#[test]
fn invalid_paseo_owner_fails_before_any_commit() {
    let mut store = Store::in_memory().unwrap();
    for owner in [
        "",
        "../other-account",
        "AAAAAAAAAAAAAAAAAAAAAAAA",
        "too-short",
    ] {
        let bytes =
            serde_json::to_vec(&json!({"owner":owner,"conversations":[{"id":"one","mapping":{}}]}))
                .unwrap();
        assert!(importer::import_paseo_history(&mut store, &bytes).is_err());
        assert_eq!(store.library_revision().unwrap(), 0);
    }
}

#[test]
fn cached_transcripts_are_searchable_archives_and_do_not_claim_original_message_graphs() {
    let mut store = Store::in_memory().unwrap();
    let text =
        "# Saved chat\n\n## You\n\ncached unique phrase\n\n---\n\n## ChatGPT\n\nA saved reply.";
    let bytes = serde_json::to_vec(&json!({
        "owner":"aaaaaaaaaaaaaaaaaaaaaaaa", "format":"paseo-cached-transcripts-v1",
        "conversations":[{"id":"conversation:one","kind":"conversation","source":"chatgpt",
            "title":"Saved conversation","text":text,"meta":"{}","url":"https://chatgpt.com/c/one"}]
    }))
    .unwrap();
    assert_eq!(
        importer::import_paseo_history(&mut store, &bytes)
            .unwrap()
            .imported,
        1
    );
    assert_eq!(
        importer::import_paseo_history(&mut store, &bytes)
            .unwrap()
            .unchanged,
        1
    );
    let rows = store.library_snapshot().unwrap().conversations;
    assert_eq!(rows[0]["provider"], "paseo-chatgpt");
    assert_eq!(
        rows[0]["account_scope"],
        "paseo-cache:aaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(store.search("cached unique", 10).unwrap().len(), 1);
    let detail = store
        .read_normalized(rows[0]["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    let serialized = serde_json::to_string(&detail).unwrap();
    assert!(serialized.contains("archive"));
    assert!(serialized.contains("paseo-cached-transcript"));
}
