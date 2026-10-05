use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use seekr::api::client::ApiClient;
use seekr::config::ProviderConfig;
use seekr::jockey::{Worker, WorkerError, WorkerPrompt};

fn worker_for(server: &MockServer) -> Worker {
    let provider = ProviderConfig {
        name: "test-worker".to_string(),
        key: String::new(),
        base_url: server.uri(),
        model: "test-model".to_string(),
        timeout: Some(5),
    };
    Worker::new(
        ApiClient::new_for_provider(&Default::default(), &provider),
        "test-model".to_string(),
        0.2,
        1024,
        None,
    )
}

fn prompt() -> WorkerPrompt {
    WorkerPrompt {
        goal: "add health endpoint".into(),
        step_id: "s1".into(),
        step_description: "add handler".into(),
        invariants: vec!["handler exists".into()],
        allowed_paths: vec!["src/".into()],
        attempt: 1,
        max_attempts: 3,
        context: vec![],
    }
}

fn completion_body(content: &str, tool_calls: serde_json::Value) -> serde_json::Value {
    json!({
        "choices": [{"message": {"role": "assistant", "content": content, "tool_calls": tool_calls}}],
        "usage": {"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60}
    })
}

#[tokio::test]
async fn worker_parses_tool_call_and_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({"model": "test-model", "stream": false})))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion_body(
            "",
            json!([{"id": "call_1", "type": "function", "function": {
                "name": "edit_file",
                "arguments": "{\"path\": \"src/lib.rs\", \"old_string\": \"a\", \"new_string\": \"b\"}"
            }}]),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let turn = worker_for(&server).propose(&prompt()).await.unwrap();
    assert_eq!(turn.actions.len(), 1);
    assert_eq!(turn.actions[0].tool, "edit_file");
    assert_eq!(turn.actions[0].args["path"], "src/lib.rs");
    assert_eq!(turn.usage.unwrap().total_tokens, 60);
}

#[tokio::test]
async fn worker_repairs_invalid_tool_arguments_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion_body(
            "",
            json!([{"id": "call_1", "type": "function", "function": {
                "name": "write_file",
                "arguments": "{path: src/x.rs, content: broken"  // invalid JSON
            }}]),
        )))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion_body(
            "",
            json!([{"id": "call_2", "type": "function", "function": {
                "name": "write_file",
                "arguments": "{\"path\": \"src/x.rs\", \"content\": \"fn f() {}\"}"
            }}]),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let turn = worker_for(&server).propose(&prompt()).await.unwrap();
    assert_eq!(turn.actions[0].args["content"], "fn f() {}");
}

#[tokio::test]
async fn worker_rejects_unknown_tool() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion_body(
            "",
            json!([{"id": "call_1", "type": "function", "function": {
                "name": "delete_everything",
                "arguments": "{}"
            }}]),
        )))
        .mount(&server)
        .await;

    let err = worker_for(&server).propose(&prompt()).await.unwrap_err();
    assert!(matches!(err, WorkerError::UnknownTool(_)));
}

#[tokio::test]
async fn worker_errors_when_no_tool_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion_body(
            "I would rather explain than act.",
            json!([]),
        )))
        .mount(&server)
        .await;

    let err = worker_for(&server).propose(&prompt()).await.unwrap_err();
    assert!(matches!(err, WorkerError::NoAction(_)));
}

#[tokio::test]
#[ignore = "live: requires SEEKR_JJ_TEST_URL (e.g. http://server:11434/v1) and SEEKR_JJ_TEST_MODEL"]
async fn live_worker_against_local_model() {
    let url = std::env::var("SEEKR_JJ_TEST_URL").unwrap();
    let model = std::env::var("SEEKR_JJ_TEST_MODEL").unwrap();
    let provider = ProviderConfig {
        name: "live".to_string(),
        key: String::new(),
        base_url: url,
        model: model.clone(),
        timeout: Some(300),
    };
    let worker = Worker::new(
        ApiClient::new_for_provider(&Default::default(), &provider),
        model,
        0.2,
        8192,
        Some("none".to_string()),
    );
    let prompt = WorkerPrompt {
        goal: "Verify the workspace tooling".into(),
        step_id: "probe".into(),
        step_description: "List the files in the repository root using the read tool".into(),
        invariants: vec!["one read_file call is proposed".into()],
        allowed_paths: vec!["src/".into()],
        attempt: 1,
        max_attempts: 3,
        context: vec![],
    };
    let turn = worker.propose(&prompt).await.unwrap();
    println!("actions: {:?}", turn.actions);
    println!("usage: {:?}", turn.usage);
    assert_eq!(turn.actions.len(), 1);
    assert_eq!(turn.actions[0].tool, "read_file");
    assert!(turn.actions[0].args.get("path").is_some());
}
