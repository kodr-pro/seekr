use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use seekr::jev::client::JevConfig;
use seekr::jev::questions::{choice, instructions, noul, score};
use seekr::jev::{JevClient, JevError, QuestionSet};

fn sample_set() -> QuestionSet {
    let mut criteria = BTreeMap::new();
    criteria.insert("syntax_fix".to_string(), json!("compiler error"));
    criteria.insert("read_context".to_string(), json!("missing context"));
    criteria.insert("deadlock".to_string(), json!("architectural block"));
    QuestionSet::new()
        .add(
            "scope",
            noul(
                instructions("Is the edit within the step?", "Step governs."),
                Some(json!("stays within")),
                Some(json!("strays beyond")),
            ),
        )
        .add("triage", choice(instructions("Pick.", "One."), criteria))
        .add(
            "novelty",
            score(
                instructions("Novel?", "vs prior failures"),
                vec![
                    json!("identical"),
                    json!("cosmetic"),
                    json!("partial"),
                    json!("novel"),
                ],
            )
            .unwrap(),
        )
}

fn wire_body() -> Value {
    json!({
        "model": "jev-test",
        "answers": {
            "scope": {"type": "noul", "noul": 0.91},
            "triage": {"type": "choice", "choice": "syntax_fix", "confidence": 0.82,
                       "probabilities": {"syntax_fix": 0.82, "read_context": 0.10, "deadlock": 0.08}},
            "novelty": {"type": "score", "score": 2.4, "confidence": 0.7,
                        "legend": {"0": "identical", "1": "cosmetic", "2": "partial", "3": "novel"},
                        "probabilities": {"0": 0.1, "1": 0.05, "2": 0.4, "3": 0.45}}
        },
        "usage": {"input_tokens": 949, "output_tokens": 83}
    })
}

fn client_for(
    server: &MockServer,
    cache: Option<std::path::PathBuf>,
) -> JevClient {
    let cfg = JevConfig {
        base_url: server.uri(),
        api_key: "ts-test-key".to_string(),
        model: "jev-test".to_string(),
        egress_enabled: true,
        timeout: Duration::from_secs(5),
        max_retries: 2,
        cache_dir: cache,
    };
    JevClient::new(cfg)
}

#[tokio::test]
async fn ask_batches_all_questions_and_parses_answers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_partial_json(json!({
            "model": "jev-test",
            "questions": {
                "scope": {"type": "noul"},
                "triage": {"type": "choice"},
                "novelty": {"type": "score"}
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_body()))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server, None);
    let result = client
        .ask(
            &json!({"step": "add tests", "tool": "write_file"}),
            &sample_set(),
        )
        .await
        .unwrap();

    assert!(!result.cached);
    assert_eq!(result.model, "jev-test");
    assert_eq!(result.answers["scope"].as_noul(), Some(0.91));
    let (choice, confidence) = result.answers["triage"].as_choice().unwrap();
    assert_eq!(choice, "syntax_fix");
    assert!((confidence - 0.82).abs() < 1e-9);
    let (score_val, _) = result.answers["novelty"].as_score().unwrap();
    assert!((score_val - 2.4).abs() < 1e-9);
    assert_eq!(result.usage.input_tokens, 949);
    assert_eq!(result.usage.output_tokens, 83);
}

#[tokio::test]
async fn ask_retries_on_500_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string("upstream boom"),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_body()))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server, None);
    let result = client.ask(&json!({"n": 1}), &sample_set()).await;
    assert!(result.is_ok(), "expected success after retrying 500s");
}

#[tokio::test]
async fn ask_fails_closed_on_4xx() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server, None);
    let err = client.ask(&json!({}), &sample_set()).await.unwrap_err();
    assert!(
        matches!(err, JevError::HttpStatus(code, _) if code.as_u16() == 401)
    );
}

#[tokio::test]
async fn ask_uses_cache_on_second_identical_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(wire_body()))
        .expect(1)
        .mount(&server)
        .await;

    let cache = tempfile::tempdir().unwrap();
    let client = client_for(&server, Some(cache.path().to_path_buf()));
    let state = json!({"step": "cache probe"});

    let first = client.ask(&state, &sample_set()).await.unwrap();
    assert!(!first.cached);
    let second = client.ask(&state, &sample_set()).await.unwrap();
    assert!(second.cached);
    assert_eq!(first.answers, second.answers);
}

#[tokio::test]
async fn ask_rejects_answer_count_mismatch() {
    let server = MockServer::start().await;
    let mut body = wire_body();
    body["answers"]
        .as_object_mut()
        .unwrap()
        .remove("novelty")
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server, None);
    let err = client.ask(&json!({}), &sample_set()).await.unwrap_err();
    assert!(matches!(err, JevError::InvalidResponse(_)));
}

#[tokio::test]
#[ignore = "live: requires real TYPESAFE_API_KEY and network egress"]
async fn live_smoke_against_typesafe() {
    let client = JevClient::from_env();
    assert!(
        client.unavailable().is_none(),
        "TYPESAFE_API_KEY must be set"
    );
    let set = QuestionSet::new().add(
        "smoke",
        noul(
            instructions("Is 2+2 equal to 4?", "Arithmetic."),
            None,
            None,
        ),
    );
    let result = client
        .ask(&json!({"probe": "seekr smoke test"}), &set)
        .await
        .unwrap();
    let p = result.answers["smoke"].as_noul().unwrap();
    assert!(p > 0.5, "expected high P(yes), got {p}");
    println!(
        "model={} p={p} usage={}+{}",
        result.model, result.usage.input_tokens, result.usage.output_tokens
    );
}

#[tokio::test]
async fn ask_rejects_out_of_range_probability() {
    let server = MockServer::start().await;
    let mut body = wire_body();
    body["answers"]["scope"]["noul"] = json!(1.7);
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server, None);
    let err = client.ask(&json!({}), &sample_set()).await.unwrap_err();
    assert!(matches!(err, JevError::InvalidResponse(_)));
}
