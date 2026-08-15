use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn reads_utf8_safe_token_windows() {
    let store = ToolOutputStore::new().unwrap();
    let stored = store
        .store_text("😀😀😀😀😀 middle 😀😀😀😀😀".to_string())
        .await
        .unwrap();

    let first = store
        .read(&stored.handle, /*offset*/ 0, /*max_tokens*/ 2)
        .await
        .unwrap();
    assert_eq!(first.content, "😀😀");
    assert_eq!(first.next_offset, Some(2));

    let second = store
        .read(&stored.handle, /*offset*/ 2, /*max_tokens*/ 3)
        .await
        .unwrap();
    assert_eq!(second.content, "😀😀😀");
    assert_eq!(second.next_offset, Some(5));
}

#[tokio::test]
async fn searches_literal_matches_with_bounded_context() {
    let store = ToolOutputStore::new().unwrap();
    let stored = store
        .store_text("zero\nfirst needle\ntwo\nsecond needle\nfour\n".to_string())
        .await
        .unwrap();

    let result = store
        .search(
            &stored.handle,
            "needle",
            /*context_lines*/ 1,
            /*max_tokens*/ 100,
        )
        .await
        .unwrap();

    assert_eq!(result.total_matches, 2);
    assert!(!result.truncated);
    assert_eq!(
        result.matches,
        vec![
            ToolOutputSearchMatch {
                line_number: 2,
                context_start_line: 1,
                context_end_line: 3,
                content: "zero\nfirst needle\ntwo".to_string(),
            },
            ToolOutputSearchMatch {
                line_number: 4,
                context_start_line: 3,
                context_end_line: 5,
                content: "two\nsecond needle\nfour".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn rejects_invalid_or_foreign_handles_without_disclosing_contents() {
    let store = ToolOutputStore::new().unwrap();
    let other_store = ToolOutputStore::new().unwrap();
    let stored = store.store_text("secret".to_string()).await.unwrap();

    assert_eq!(
        other_store
            .read(&stored.handle, /*offset*/ 0, /*max_tokens*/ 1)
            .await,
        Err(ToolOutputStoreError::Unavailable)
    );
    assert_eq!(
        store
            .search(
                &stored.handle,
                "",
                /*context_lines*/ 0,
                /*max_tokens*/ 1
            )
            .await,
        Err(ToolOutputStoreError::EmptyPattern)
    );
}

#[tokio::test]
async fn enforces_entry_and_session_quotas() {
    let store = ToolOutputStore::new_with_limits(ToolOutputStoreLimits {
        max_outputs: 1,
        max_output_bytes: 4,
        max_total_output_bytes: 6,
    })
    .unwrap();

    assert_eq!(
        store.store_text("12345".to_string()).await,
        Err(ToolOutputStoreError::OutputTooLarge)
    );
    store.store_text("1234".to_string()).await.unwrap();
    assert_eq!(
        store.store_text("12".to_string()).await,
        Err(ToolOutputStoreError::StorageFull)
    );
}

#[tokio::test]
async fn removes_backing_files_when_the_session_store_drops() {
    let store = ToolOutputStore::new().unwrap();
    let stored = store.store_text("content".to_string()).await.unwrap();
    let path = {
        let state = store.state.lock().await;
        state.outputs[&stored.handle].path.clone()
    };
    assert!(path.exists());

    drop(store);
    assert!(!path.exists());
}

#[tokio::test]
async fn stream_capture_flushes_output_in_order() {
    let store = ToolOutputStore::new().unwrap();
    let stream = store.start_stream().await.unwrap();
    stream.append(b"first\n".to_vec()).await;
    stream.append(b"second\n".to_vec()).await;

    let stored = stream
        .recoverable_output()
        .await
        .expect("stream capture should remain available");
    let output = store.read(&stored.handle, 0, 10).await.unwrap();
    assert_eq!(output.content, "first\nsecond\n");
}
