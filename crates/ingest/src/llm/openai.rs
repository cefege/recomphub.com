//! OpenAI Responses API, with an API key or with a ChatGPT Plus/Pro Codex token.
//!
//! The Codex path follows OMP's Codex provider (`packages/ai/src/providers/openai-codex-responses.ts`,
//! `openai-codex/request-transformer.ts`): the same endpoint, headers, and SSE stream the Codex CLI
//! uses. OpenAI and Codex both reject `minLength`/`maxLength` in strict schemas, so those are
//! stripped here; the caller's own validation still enforces the limits.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use super::{Request, json_reply};

const API_URL: &str = "https://api.openai.com/v1/responses";
const CODEX_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
/// Codex client version (OMP `CODEX_CLIENT_VERSION`).
const CODEX_CLIENT_VERSION: &str = "0.159.0";

/// Calls the Responses API with an API key.
pub async fn complete_api_key(
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    req: &Request<'_>,
) -> Result<Value> {
    let body = json!({
        "model": model,
        "instructions": req.system,
        "input": [{"role": "user", "content": req.user}],
        "text": {"format": json_schema(req)},
        "max_output_tokens": req.max_tokens,
        "store": false,
    });
    let reply = http
        .post(API_URL)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .context("calling the OpenAI Responses API")?;
    let reply = json_reply(reply, "OpenAI Responses API").await?;
    parse_model_json(&output_text(&reply)?)
}

/// Calls the ChatGPT Codex backend with a ChatGPT OAuth token and its account id.
pub async fn complete_codex(
    http: &reqwest::Client,
    access_token: &str,
    account_id: &str,
    model: &str,
    req: &Request<'_>,
) -> Result<Value> {
    let body = json!({
        "model": model,
        "instructions": req.system,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": req.user}],
        }],
        "text": {"format": json_schema(req)},
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });
    let session = random_hex();
    let reply = http
        .post(CODEX_URL)
        .bearer_auth(access_token)
        .header("chatgpt-account-id", account_id)
        .header("OpenAI-Beta", "responses=experimental")
        .header("originator", "omp")
        .header("version", CODEX_CLIENT_VERSION)
        .header("session_id", &session)
        .header("conversation_id", &session)
        .header("x-codex-routing-hint", format!("model={model}"))
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&body)
        .send()
        .await
        .context("calling the ChatGPT Codex backend")?;
    let status = reply.status();
    let text = reply
        .text()
        .await
        .context("reading the ChatGPT Codex response")?;
    if !status.is_success() {
        let detail: String = text.chars().take(300).collect();
        bail!("ChatGPT Codex backend returned {status}: {detail}");
    }
    parse_model_json(&codex_sse_text(&text)?)
}

/// Structured-output format for the record. OpenAI and Codex reject length limits in strict
/// schemas, so they are removed; `strict` is off so the remaining schema is accepted as is.
fn json_schema(req: &Request<'_>) -> Value {
    json!({
        "type": "json_schema",
        "name": req.tool_name,
        "description": req.tool_description,
        "schema": strip_length_limits(req.schema),
        "strict": false,
    })
}

/// Copy of `schema` without `minLength` and `maxLength`, at every depth.
pub fn strip_length_limits(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "minLength" && key.as_str() != "maxLength")
                .map(|(key, value)| (key.clone(), strip_length_limits(value)))
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(strip_length_limits).collect()),
        other => other.clone(),
    }
}

/// Text of the first `message` output of a non-streamed Responses reply.
fn output_text(reply: &Value) -> Result<String> {
    reply["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "message")
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter(|part| part["type"] == "output_text")
        .find_map(|part| part["text"].as_str().map(str::to_string))
        .context("model returned no output text")
}

/// Reads the answer from a Codex SSE stream. Deltas are joined; when none arrive, the text is
/// taken from the completed item or the final response. `error` and `response.failed` events
/// are returned as errors.
pub fn codex_sse_text(stream: &str) -> Result<String> {
    let mut deltas = String::new();
    let mut from_items = String::new();
    for line in stream.lines() {
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let event: Value =
            serde_json::from_str(payload).context("parsing a ChatGPT Codex stream event")?;
        match event["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => {
                if let Some(delta) = event["delta"].as_str() {
                    deltas.push_str(delta);
                }
            }
            "response.output_item.done" => {
                if event["item"]["type"] == "message" {
                    from_items.push_str(&message_text(&event["item"]));
                }
            }
            "response.completed" | "response.done" | "response.incomplete" => {
                let final_text = event["response"]["output"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(message_text)
                    .collect::<String>();
                if from_items.is_empty() {
                    from_items = final_text;
                }
            }
            "error" => {
                let message = event["message"].as_str().unwrap_or("unknown error");
                bail!("ChatGPT Codex stream error: {message}");
            }
            "response.failed" => {
                let message = event["response"]["error"]["message"]
                    .as_str()
                    .unwrap_or("response failed");
                bail!("ChatGPT Codex response failed: {message}");
            }
            _ => {}
        }
    }
    let text = if deltas.is_empty() {
        from_items
    } else {
        deltas
    };
    if text.is_empty() {
        bail!("ChatGPT Codex returned no output text");
    }
    Ok(text)
}

/// Concatenated `output_text` parts of one message item.
fn message_text(item: &Value) -> String {
    if item["type"] != "message" {
        return String::new();
    }
    item["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "output_text")
        .filter_map(|part| part["text"].as_str())
        .collect()
}

fn parse_model_json(text: &str) -> Result<Value> {
    serde_json::from_str(text).context("model output is not the requested JSON")
}

/// Random 16-byte session id, lowercase hex. Codex uses one per conversation.
fn random_hex() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_length_limits_removes_only_string_bounds() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["title", "tags"],
            "properties": {
                "title": {"type": "string", "minLength": 1, "maxLength": 120},
                "tags": {"type": "array", "maxItems": 5, "items": {"type": "string", "maxLength": 40}},
            },
        });
        let stripped = strip_length_limits(&schema);
        let text = stripped.to_string();
        assert!(!text.contains("minLength"));
        assert!(!text.contains("maxLength\":120"));
        assert!(!text.contains("maxLength\":40"));
        assert_eq!(stripped["properties"]["tags"]["maxItems"], 5);
        assert_eq!(stripped["additionalProperties"], false);
        assert_eq!(stripped["required"], json!(["title", "tags"]));
        assert_eq!(stripped["properties"]["title"]["type"], "string");
    }

    #[test]
    fn codex_stream_joins_deltas() {
        let stream = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"a\\\":\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"1}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n",
            "data: [DONE]\n",
        );
        assert_eq!(codex_sse_text(stream).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn codex_stream_falls_back_to_completed_items() {
        let stream = concat!(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"{\\\"b\\\":2}\"}]}}\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n",
        );
        assert_eq!(codex_sse_text(stream).unwrap(), "{\"b\":2}");
    }

    #[test]
    fn codex_stream_takes_text_from_the_final_response() {
        let stream = "data: {\"type\":\"response.done\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"{}\"}]}]}}\n";
        assert_eq!(codex_sse_text(stream).unwrap(), "{}");
    }

    #[test]
    fn codex_stream_reports_errors() {
        let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"quota\"}}}\n";
        assert!(
            codex_sse_text(failed)
                .unwrap_err()
                .to_string()
                .contains("quota")
        );
        let error = "data: {\"type\":\"error\",\"message\":\"bad\"}\n";
        assert!(
            codex_sse_text(error)
                .unwrap_err()
                .to_string()
                .contains("bad")
        );
        assert!(codex_sse_text("data: [DONE]\n").is_err());
    }

    #[test]
    fn output_text_reads_the_first_message_text() {
        let reply = json!({"output": [
            {"type": "reasoning", "summary": []},
            {"type": "message", "content": [{"type": "output_text", "text": "{\"c\":3}"}]},
        ]});
        assert_eq!(output_text(&reply).unwrap(), "{\"c\":3}");
        assert!(output_text(&json!({"output": []})).is_err());
    }
}
