//! Anthropic Messages API, with an API key or with a Claude Pro/Max OAuth token.
//!
//! The OAuth path sends the headers and system block layout Claude Code sends. Versions and
//! beta flags are pinned to what OMP ships (`packages/ai/src/providers/anthropic.ts`,
//! `omp_cc_fp.ts`); Anthropic may change what it accepts, so update the constants here first.

use anyhow::{Context, Result};
use reqwest::RequestBuilder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use xxhash_rust::xxh64::xxh64;

use super::{Request, json_reply};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Claude Code's inference endpoint carries `?beta=true`.
const OAUTH_URL: &str = "https://api.anthropic.com/v1/messages?beta=true";
const API_VERSION: &str = "2023-06-01";

/// Claude Code version in the User-Agent and billing header (OMP `DEFAULT_CLAUDE_CODE_VERSION`).
pub const CLAUDE_CODE_VERSION: &str = "2.1.280";
const STAINLESS_PACKAGE_VERSION: &str = "0.112.1";
const STAINLESS_RUNTIME_VERSION: &str = "v26.3.0";
const CLAUDE_CODE_SYSTEM: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const BILLING_PREFIX: &str = "x-anthropic-billing-header:";
const BILLING_SALT: &str = "59cf53e54c78";
const CCH_PLACEHOLDER: &[u8] = b"cch=00000";
const CCH_SEED: u64 = 0x4d65_9218_e32a_3268;
/// Betas on a Claude Code utility request (OMP `claudeCodeUtilityBetaDefaults`).
const OAUTH_BETAS: [&str; 6] = [
    "oauth-2025-04-20",
    "interleaved-thinking-2025-05-14",
    "thinking-token-count-2026-05-13",
    "context-management-2025-06-27",
    "prompt-caching-scope-2026-01-05",
    "structured-outputs-2025-12-15",
];

/// Calls the Messages API with an API key.
pub async fn complete_api_key(
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    req: &Request<'_>,
) -> Result<Value> {
    let body = json!({
        "model": model,
        "max_tokens": req.max_tokens,
        "system": req.system,
        "tools": [tool(req)],
        "tool_choice": {"type": "tool", "name": req.tool_name},
        "messages": [{"role": "user", "content": req.user}],
    });
    let reply = http
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .json(&body)
        .send()
        .await
        .context("calling the Anthropic Messages API")?;
    tool_input(
        &json_reply(reply, "Anthropic Messages API").await?,
        req.tool_name,
    )
}

/// Calls the Messages API with a Claude Pro/Max OAuth access token, as Claude Code does.
pub async fn complete_oauth(
    http: &reqwest::Client,
    access_token: &str,
    model: &str,
    req: &Request<'_>,
) -> Result<Value> {
    let body = json!({
        "model": model,
        "max_tokens": req.max_tokens,
        "system": [
            {"type": "text", "text": billing_text(req.user)},
            {"type": "text", "text": CLAUDE_CODE_SYSTEM},
            {"type": "text", "text": req.system},
        ],
        "tools": [tool(req)],
        "tool_choice": {"type": "tool", "name": req.tool_name},
        "messages": [{"role": "user", "content": req.user}],
    });
    let bytes = signed_body(&body)?;
    let betas = OAUTH_BETAS.join(",");
    let reply = claude_code_headers(http.post(OAUTH_URL), access_token, &betas)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(bytes)
        .send()
        .await
        .context("calling the Anthropic Messages API")?;
    tool_input(
        &json_reply(reply, "Anthropic Messages API").await?,
        req.tool_name,
    )
}

fn tool(req: &Request<'_>) -> Value {
    json!({
        "name": req.tool_name,
        "description": req.tool_description,
        "input_schema": req.schema,
    })
}

/// Headers Claude Code's CLI sends (OMP `claudeCodeHeaders`). `Accept-Encoding` is left to
/// reqwest, which decodes the gzip it advertises.
fn claude_code_headers(request: RequestBuilder, token: &str, betas: &str) -> RequestBuilder {
    request
        .header(
            "user-agent",
            format!("claude-cli/{CLAUDE_CODE_VERSION} (external, cli)"),
        )
        .header("accept", "application/json")
        .header("anthropic-version", API_VERSION)
        .header("anthropic-dangerous-direct-browser-access", "true")
        .header("authorization", format!("Bearer {token}"))
        .header("x-app", "cli")
        .header("anthropic-beta", betas)
        .header("x-stainless-arch", stainless_arch())
        .header("x-stainless-lang", "js")
        .header("x-stainless-os", stainless_os())
        .header("x-stainless-package-version", STAINLESS_PACKAGE_VERSION)
        .header("x-stainless-retry-count", "0")
        .header("x-stainless-runtime", "node")
        .header("x-stainless-runtime-version", STAINLESS_RUNTIME_VERSION)
        .header("x-stainless-timeout", "600")
}

/// Stainless arch names (OMP `mapStainlessArch`).
fn stainless_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x64".to_string(),
        "aarch64" => "arm64".to_string(),
        "x86" => "x86".to_string(),
        other => format!("other::{other}"),
    }
}

/// Stainless OS names (OMP `mapStainlessOs`).
fn stainless_os() -> String {
    match std::env::consts::OS {
        "macos" => "MacOS".to_string(),
        "windows" => "Windows".to_string(),
        "linux" => "Linux".to_string(),
        "freebsd" => "FreeBSD".to_string(),
        other => format!("Other::{other}"),
    }
}

/// First system block Claude Code sends (OMP `createClaudeBillingHeader`). The three sampled
/// characters come from the first user message, as UTF-16 code units; missing ones count as "0".
/// Lone surrogates become U+FFFD, which is what Node's UTF-8 encoder writes for them.
pub fn billing_text(first_user: &str) -> String {
    let units: Vec<u16> = first_user.encode_utf16().collect();
    let sampled: String = [4usize, 7, 20]
        .into_iter()
        .map(|i| match units.get(i) {
            Some(unit) => String::from_utf16_lossy(&[*unit]),
            None => "0".to_string(),
        })
        .collect();
    let digest = Sha256::digest(format!("{BILLING_SALT}{sampled}{CLAUDE_CODE_VERSION}").as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{BILLING_PREFIX} cc_version={CLAUDE_CODE_VERSION}.{suffix}; cc_entrypoint=cli; cch=00000;",
        suffix = &hex[..3],
    )
}

/// Serializes the body and fills in the `cch` integrity value (OMP `patchCch`). The value is the
/// low 20 bits of XXH64 over the whole body with the `00000` placeholder still in place, written
/// as five lowercase hex digits. The bytes returned are exactly the bytes that were hashed.
fn signed_body(body: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(body).context("serializing the request body")?;
    let prefix =
        find(&bytes, BILLING_PREFIX.as_bytes()).context("billing header missing from request")?;
    let window_end = (prefix + 256).min(bytes.len());
    let placeholder = find(&bytes[prefix..window_end], CCH_PLACEHOLDER)
        .context("cch placeholder missing from billing header")?
        + prefix;
    let digits = placeholder + 4;
    let cch = format!("{:05x}", xxh64(&bytes, CCH_SEED) & 0xf_ffff);
    bytes[digits..digits + 5].copy_from_slice(cch.as_bytes());
    Ok(bytes)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The input of the forced tool call.
fn tool_input(reply: &Value, tool_name: &str) -> Result<Value> {
    reply["content"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|b| b["type"] == "tool_use" && b["name"] == tool_name)
        .map(|b| b["input"].clone())
        .with_context(|| format!("model did not call {tool_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn billing_text_has_the_claude_code_shape() {
        let text = billing_text("Describe this project.");
        let prefix = format!("{BILLING_PREFIX} cc_version={CLAUDE_CODE_VERSION}.");
        let rest = text.strip_prefix(&prefix).expect("prefix");
        assert_eq!(&rest[3..], "; cc_entrypoint=cli; cch=00000;");
        assert!(rest[..3].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn billing_suffix_follows_the_sampled_characters() {
        assert_eq!(billing_text("x"), billing_text("x"));
        assert_ne!(
            billing_text("aaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            billing_text("bbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        );
    }

    #[test]
    fn signed_body_writes_the_hash_of_the_unsigned_body() {
        let body = json!({
            "model": "m",
            "system": [{"type": "text", "text": billing_text("hello world, this is a prompt")}],
        });
        let unsigned = serde_json::to_vec(&body).unwrap();
        let signed = signed_body(&body).unwrap();
        assert_eq!(signed.len(), unsigned.len());
        let at = find(&unsigned, CCH_PLACEHOLDER).unwrap() + 4;
        let expected = format!("{:05x}", xxh64(&unsigned, CCH_SEED) & 0xf_ffff);
        assert_eq!(expected.len(), 5);
        assert_eq!(&signed[at..at + 5], expected.as_bytes());
        assert_eq!(
            &signed[..at],
            &unsigned[..at],
            "bytes before the value are unchanged"
        );
        let text = String::from_utf8(signed).unwrap();
        assert!(text.contains("cch="));
    }

    #[test]
    fn signed_body_rejects_a_body_without_the_billing_header() {
        assert!(signed_body(&json!({"system": "plain"})).is_err());
    }

    #[test]
    fn tool_input_finds_the_forced_tool_call() {
        let reply = json!({"content": [
            {"type": "text", "text": "ok"},
            {"type": "tool_use", "name": "record_description", "input": {"summary": "s"}},
        ]});
        assert_eq!(
            tool_input(&reply, "record_description").unwrap(),
            json!({"summary": "s"})
        );
        assert!(tool_input(&json!({"content": []}), "record_description").is_err());
    }
}
