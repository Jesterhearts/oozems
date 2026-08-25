use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde::Serialize;
use url::Url;

use crate::text::truncate;

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Clone, Debug, Serialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug)]
pub struct CompletionConfig<'a> {
    pub base_url: &'a str,
    pub api_key: &'a str,
    pub model: &'a str,
    pub maximum_tokens: u32,
    pub reasoning: Option<ReasoningConfig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ReasoningConfig {
    pub effort: ReasoningEffort,
    pub exclude: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

pub fn client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(3 * 60))
        .build()
        .context("failed to build the HTTP client")
}

pub fn completion_url(base_url: &str) -> Result<Url> {
    let mut url = Url::parse(base_url).context("model provider base URL is invalid")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("model provider base URL must be an HTTP or HTTPS URL with a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("model provider base URL cannot contain credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("model provider base URL cannot contain a query or fragment");
    }
    let path = format!("{}/chat/completions", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url)
}

pub fn is_default_openrouter(base_url: &str) -> bool {
    base_url.trim_end_matches('/') == DEFAULT_BASE_URL
}

pub fn complete(
    client: &Client,
    config: &CompletionConfig<'_>,
    messages: &[Message],
) -> Result<String> {
    let url = completion_url(config.base_url)?;
    let response = client
        .post(url)
        .bearer_auth(config.api_key)
        .json(&CompletionRequest {
            model: config.model,
            messages,
            temperature: 0.0,
            maximum_tokens: config.maximum_tokens,
            reasoning: config.reasoning,
            stream: false,
        })
        .send()
        .context("model provider request failed")?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "response body could not be read".to_owned());
        let message = serde_json::from_str::<ProviderError>(&body)
            .ok()
            .map(|error| error.error.message)
            .unwrap_or_else(|| truncate(&body, 2_000));
        bail!("model provider returned {status}: {message}");
    }
    let completion = response
        .json::<CompletionResponse>()
        .context("model provider returned an invalid chat completion")?;
    completion_text(completion, config.maximum_tokens)
}

fn completion_text(
    completion: CompletionResponse,
    maximum_tokens: u32,
) -> Result<String> {
    let choice = completion
        .choices
        .into_iter()
        .next()
        .context("model provider returned no completion choice")?;
    if choice.finish_reason.as_deref() == Some("length") {
        bail!(
            "model completion reached the {maximum_tokens}-token limit; raise --max-tokens or \
             reduce --reasoning-effort"
        );
    }
    let content = choice
        .message
        .content
        .context("model provider returned no text completion")?;
    if content.trim().is_empty() {
        bail!("model provider returned an empty text completion");
    }
    Ok(content)
}

#[derive(Serialize)]
struct CompletionRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    temperature: f32,
    #[serde(rename = "max_tokens")]
    maximum_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig>,
    stream: bool,
}

#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<CompletionChoice>,
}

#[derive(Deserialize)]
struct CompletionChoice {
    message: CompletionMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct CompletionMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ProviderError {
    error: ProviderErrorBody,
}

#[derive(Deserialize)]
struct ProviderErrorBody {
    message: String,
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    #[test]
    fn completion_endpoint_accepts_compatible_http_urls() {
        assert_eq!(
            completion_url("http://localhost:11434/v1/")
                .expect("local compatible URL")
                .as_str(),
            "http://localhost:11434/v1/chat/completions"
        );
        assert!(completion_url("file:///tmp/model").is_err());
        assert!(completion_url("https://user:secret@example.com/v1").is_err());
        assert!(completion_url("https://example.com/v1?route=other").is_err());
        assert!(completion_url("https://example.com/v1#fragment").is_err());
    }

    #[test]
    fn only_the_exact_default_uses_stored_openrouter_credentials() {
        assert!(is_default_openrouter(DEFAULT_BASE_URL));
        assert!(is_default_openrouter(&format!("{DEFAULT_BASE_URL}/")));
        assert!(!is_default_openrouter("https://example.com/api/v1"));
    }

    #[test]
    fn completion_uses_the_openai_compatible_http_contract() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test listener");
        let address = listener.local_addr().expect("test listener address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("provider request");
            let request = read_http_request(&mut stream);
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("request headers");
            let headers = String::from_utf8_lossy(&request[..header_end]);
            assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
            assert!(
                headers
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case("authorization: Bearer secret"))
            );
            let body = serde_json::from_slice::<serde_json::Value>(&request[header_end + 4..])
                .expect("request JSON");
            assert_eq!(body["model"], "test/model");
            assert_eq!(body["temperature"], 0.0);
            assert_eq!(body["max_tokens"], 512);
            assert_eq!(body["reasoning"]["effort"], "low");
            assert_eq!(body["reasoning"]["exclude"], true);
            assert_eq!(body["stream"], false);
            assert_eq!(body["messages"][0]["role"], "user");
            assert_eq!(body["messages"][0]["content"], "quest evidence");

            let body = r#"{"choices":[{"message":{"content":"[[scripts]]\nname = \"q1e\""}}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                 {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("provider response");
        });
        let base_url = format!("http://{address}/v1");
        let response = complete(
            &client().expect("HTTP client"),
            &CompletionConfig {
                base_url: &base_url,
                api_key: "secret",
                model: "test/model",
                maximum_tokens: 512,
                reasoning: Some(ReasoningConfig {
                    effort: ReasoningEffort::Low,
                    exclude: true,
                }),
            },
            &[Message {
                role: MessageRole::User,
                content: "quest evidence".to_owned(),
            }],
        )
        .expect("completion");

        assert_eq!(response, "[[scripts]]\nname = \"q1e\"");
        server.join().expect("test provider server");
    }

    #[test]
    fn token_limit_finish_reason_reports_the_configured_budget() {
        let completion = serde_json::from_value::<CompletionResponse>(serde_json::json!({
            "choices": [{
                "message": { "content": "partial" },
                "finish_reason": "length",
            }],
        }))
        .expect("completion response");

        let error = completion_text(completion, 16_384).expect_err("truncated completion");

        assert!(error.to_string().contains("16384-token limit"));
        assert!(error.to_string().contains("--reasoning-effort"));
    }

    #[test]
    fn absent_reasoning_config_is_omitted_from_compatible_requests() {
        let request = CompletionRequest {
            model: "test/model",
            messages: &[],
            temperature: 0.0,
            maximum_tokens: 16_384,
            reasoning: None,
            stream: false,
        };

        let value = serde_json::to_value(request).expect("completion request");

        assert!(value.get("reasoning").is_none());
    }

    fn read_http_request(stream: &mut impl Read) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let (header_end, content_length) = loop {
            let bytes = stream.read(&mut buffer).expect("read provider request");
            assert!(bytes > 0, "provider request ended before its headers");
            request.extend_from_slice(&buffer[..bytes]);
            if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .expect("request content length");
                break (header_end, content_length);
            }
            assert!(
                request.len() <= 64 * 1024,
                "provider request headers are too long"
            );
        };
        let expected_length = header_end + 4 + content_length;
        while request.len() < expected_length {
            let bytes = stream
                .read(&mut buffer)
                .expect("read provider request body");
            assert!(bytes > 0, "provider request body ended early");
            request.extend_from_slice(&buffer[..bytes]);
        }
        request.truncate(expected_length);
        request
    }
}
