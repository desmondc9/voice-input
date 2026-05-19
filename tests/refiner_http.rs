use voice_input::refiner::LlmRefiner;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn happy_path_returns_refined_text() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-test"))
        .and(body_partial_json(serde_json::json!({
            "model": "gpt-4o-mini",
            "temperature": 0.3,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [
                {"message": {"role": "assistant", "content": "Python and JSON"}}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let refiner = LlmRefiner::for_test(server.uri(), "sk-test", "gpt-4o-mini");
    let out = refiner.try_refine("配森 和 杰森", false).await.unwrap();
    assert_eq!(out, "Python and JSON");
}

#[tokio::test]
async fn trailing_slash_in_base_url_does_not_double_up() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "ok"}}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let url_with_slash = format!("{}/", server.uri());
    let refiner = LlmRefiner::for_test(url_with_slash, "sk-test", "gpt-4o-mini");
    let out = refiner.try_refine("hi", false).await.unwrap();
    assert_eq!(out, "ok");
}

#[tokio::test]
async fn response_content_is_whitespace_trimmed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "  Python and JSON  \n"}}]
        })))
        .mount(&server)
        .await;

    let refiner = LlmRefiner::for_test(server.uri(), "sk-test", "gpt-4o-mini");
    let out = refiner.try_refine("配森", false).await.unwrap();
    assert_eq!(out, "Python and JSON");
}

#[tokio::test]
async fn disabled_refiner_short_circuits_without_request() {
    // No mocks mounted — any HTTP call would 404 and fail the assertion below.
    let server = MockServer::start().await;
    // Empty api_key marks the refiner inactive (matches Config default).
    let refiner = LlmRefiner::for_test(server.uri(), "", "gpt-4o-mini");

    let out = refiner.try_refine("hello", false).await.unwrap();
    assert_eq!(out, "hello", "inactive refiner must not contact the server");
}

#[tokio::test]
async fn force_bypasses_inactive_guard() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "forced"}}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Empty api_key → normally inactive, but force=true bypasses
    let refiner = LlmRefiner::for_test(server.uri(), "", "gpt-4o-mini");
    let out = refiner.try_refine("hi", true).await.unwrap();
    assert_eq!(out, "forced");
}

#[tokio::test]
async fn network_5xx_yields_error_from_try_refine() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("upstream down"))
        .mount(&server)
        .await;

    let refiner = LlmRefiner::for_test(server.uri(), "sk-test", "gpt-4o-mini");
    let err = refiner.try_refine("hello", false).await.unwrap_err();
    assert!(
        err.to_string().contains("non-2xx"),
        "expected non-2xx error, got: {}",
        err
    );
}

#[tokio::test]
async fn refine_falls_back_to_raw_text_on_5xx() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let refiner = LlmRefiner::for_test(server.uri(), "sk-test", "gpt-4o-mini");
    let out = refiner.refine("hello world", false).await;
    assert_eq!(
        out, "hello world",
        "refine must fall back to raw text on API errors"
    );
}

#[tokio::test]
async fn refine_falls_back_when_response_missing_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": []  // missing choices[0].message.content
        })))
        .mount(&server)
        .await;

    let refiner = LlmRefiner::for_test(server.uri(), "sk-test", "gpt-4o-mini");
    let out = refiner.refine("hello", false).await;
    assert_eq!(
        out, "hello",
        "malformed response must fall back to raw text"
    );
}
