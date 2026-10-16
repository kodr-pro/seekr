use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use seekr::api::client::ApiClient;
use seekr::config::ProviderConfig;
use seekr::jockey::{FrontierPlanner, PlanOutcome};

fn planner_for(server: &MockServer) -> FrontierPlanner {
    let provider = ProviderConfig {
        name: "test-frontier".to_string(),
        key: "sk-test".to_string(),
        base_url: server.uri(),
        model: "test-model".to_string(),
        timeout: Some(5),
    };
    FrontierPlanner::new(
        ApiClient::new_for_provider(&Default::default(), &provider),
        "test-model".to_string(),
        0.2,
        4096,
    )
}

#[tokio::test]
async fn planner_parses_clarification_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({"model": "test-model"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content":
                "{\"needs_clarification\": true, \"questions\": [{\"id\": \"q1\", \"question\": \"Which crate?\", \"why\": \"two exist\"}]}"}}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let planner = planner_for(&server);
    let response = planner
        .plan("add tests", &[], "src/\nCargo.toml")
        .await
        .unwrap();
    match response.outcome {
        PlanOutcome::NeedsClarification(qs) => {
            assert_eq!(qs.len(), 1);
            assert_eq!(qs[0].id, "q1");
        }
        PlanOutcome::Ready(_) => panic!("expected clarification"),
    }
    assert_eq!(response.usage.unwrap().total_tokens, 120);
}

#[tokio::test]
async fn planner_parses_dag_and_validates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content":
                "```json\n{\"needs_clarification\": false, \"dag\": {\"goal\": \"g\", \"steps\": [\n\
                  {\"id\": \"impl\", \"description\": \"implement\", \"depends_on\": [], \"invariants\": [\"compiles\"], \"allowed_paths\": [\"src/\"], \"verification_command\": \"cargo check\"},\n\
                  {\"id\": \"test\", \"description\": \"test\", \"depends_on\": [\"impl\"], \"invariants\": [\"tests pass\"], \"allowed_paths\": [\"tests/\"], \"verification_command\": \"cargo test\"}\n\
                ]}}\n```"}}],
            "usage": {"prompt_tokens": 200, "completion_tokens": 60, "total_tokens": 260}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let planner = planner_for(&server);
    let response = planner.plan("goal", &[], "").await.unwrap();
    match response.outcome {
        PlanOutcome::Ready(dag) => {
            assert_eq!(dag.steps.len(), 2);
            assert_eq!(dag.steps[1].depends_on, vec!["impl".to_string()]);
            assert!(dag.validate().is_ok());
        }
        PlanOutcome::NeedsClarification(_) => panic!("expected ready dag"),
    }
}

#[tokio::test]
async fn planner_repairs_malformed_first_reply() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "Sure! Here's what I'd do: first we..."}}]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content":
                "{\"needs_clarification\": false, \"dag\": {\"goal\": \"g\", \"steps\": [{\"id\": \"a\", \"description\": \"d\", \"invariants\": [\"i\"]}]}}"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let planner = planner_for(&server);
    let response = planner.plan("goal", &[], "").await.unwrap();
    assert!(matches!(response.outcome, PlanOutcome::Ready(_)));
}

#[tokio::test]
async fn planner_fails_after_repair_still_bad() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "garbage"}}]
        })))
        .mount(&server)
        .await;

    let planner = planner_for(&server);
    let err = planner.plan("goal", &[], "").await;
    assert!(err.is_err());
}

#[tokio::test]
#[ignore = "live: requires SEEKR_JJ_TEST_URL (e.g. http://server:11434/v1) and SEEKR_JJ_TEST_MODEL"]
async fn live_planner_against_local_model() {
    let url = std::env::var("SEEKR_JJ_TEST_URL").unwrap();
    let model = std::env::var("SEEKR_JJ_TEST_MODEL").unwrap();
    let provider = ProviderConfig {
        name: "live".to_string(),
        key: String::new(),
        base_url: url,
        model: model.clone(),
        timeout: Some(300),
    };
    let planner = FrontierPlanner::new(
        ApiClient::new_for_provider(&Default::default(), &provider),
        model.clone(),
        0.2,
        8192,
    );
    let tree = "Cargo.toml\nsrc/main.rs\nsrc/lib.rs\nsrc/api/\nsrc/tools/";
    let response = planner
        .plan(
            "Add a /health endpoint to the axum server that returns 200 OK.",
            &[],
            tree,
        )
        .await
        .unwrap();
    match response.outcome {
        PlanOutcome::Ready(dag) => {
            println!("{}", dag.render_summary());
            println!("usage: {:?}", response.usage);
        }
        PlanOutcome::NeedsClarification(qs) => {
            panic!("unexpected clarification: {:?}", qs);
        }
    }
}
