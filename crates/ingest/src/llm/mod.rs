//! AI providers an admin connects, and the ordered fallback between them.
//!
//! Each provider answers one structured request: a system prompt, a user message, and a JSON
//! Schema for the record to return. Adapters return that record as a `serde_json::Value`; the
//! caller checks it against its own rules.

pub mod anthropic;
pub mod oauth;
pub mod openai;

use anyhow::{Context, Result, bail};
use recomphub_db::ai_providers::{
    Credential, EnabledProvider, KIND_ANTHROPIC_API, KIND_ANTHROPIC_OAUTH, KIND_OPENAI_API,
    KIND_OPENAI_CODEX_OAUTH,
};
use recomphub_db::secrets::Sealer;
use serde_json::Value;

const ERROR_CHARS: usize = 300;

/// One structured-output request. `schema` describes the record the model must return.
pub struct Request<'a> {
    pub system: &'a str,
    pub user: &'a str,
    pub tool_name: &'a str,
    pub tool_description: &'a str,
    pub schema: &'a Value,
    pub max_tokens: u32,
}

/// Outcome of a fallback run. `answer` is the index of the provider that answered, if any.
/// `failures` lists the providers tried before it (or all of them), in order, with their errors.
pub struct Chain<T> {
    pub answer: Option<(usize, T)>,
    pub failures: Vec<(usize, String)>,
}

/// Tries each provider in order until one returns `Ok`. Stops at the first success.
pub async fn run_chain<P, T, F>(providers: &[P], mut attempt: F) -> Chain<T>
where
    F: AsyncFnMut(&P) -> Result<T>,
{
    let mut failures = Vec::new();
    for (index, provider) in providers.iter().enumerate() {
        match attempt(provider).await {
            Ok(value) => {
                return Chain {
                    answer: Some((index, value)),
                    failures,
                };
            }
            Err(e) => failures.push((index, format!("{e:#}"))),
        }
    }
    Chain {
        answer: None,
        failures,
    }
}

/// Sends one request to the provider of the given kind, with the matching credential.
pub async fn complete(
    http: &reqwest::Client,
    kind: &str,
    model: &str,
    credential: &Credential,
    req: &Request<'_>,
) -> Result<Value> {
    match (kind, credential) {
        (KIND_ANTHROPIC_API, Credential::ApiKey { api_key }) => {
            anthropic::complete_api_key(http, api_key, model, req).await
        }
        (KIND_ANTHROPIC_OAUTH, Credential::OAuth { access_token, .. }) => {
            anthropic::complete_oauth(http, access_token, model, req).await
        }
        (KIND_OPENAI_API, Credential::ApiKey { api_key }) => {
            openai::complete_api_key(http, api_key, model, req).await
        }
        (
            KIND_OPENAI_CODEX_OAUTH,
            Credential::OAuth {
                access_token,
                account_id,
                ..
            },
        ) => {
            let account = account_id
                .as_deref()
                .context("ChatGPT account id is missing; reconnect this provider")?;
            openai::complete_codex(http, access_token, account, model, req).await
        }
        _ => bail!("stored credential does not match provider kind {kind}"),
    }
}

/// Decrypts and parses the credential stored for a provider.
pub fn open_credential(sealer: &Sealer, provider: &EnabledProvider) -> Result<Credential> {
    let plain = sealer
        .open(&provider.sealed)
        .context("decrypting the provider credential")?;
    serde_json::from_slice(&plain).context("parsing the provider credential")
}

/// Encrypts a credential for storage.
pub fn seal_credential(sealer: &Sealer, credential: &Credential) -> Result<Vec<u8>> {
    let plain = serde_json::to_vec(credential).context("serializing the provider credential")?;
    Ok(sealer.seal(&plain)?)
}

/// Reads a JSON reply. Non-2xx statuses become errors that carry the provider's message.
pub(crate) async fn json_reply(response: reqwest::Response, service: &str) -> Result<Value> {
    let status = response.status();
    let text = response
        .text()
        .await
        .with_context(|| format!("reading the {service} response"))?;
    let reply: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        let detail = reply
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(text);
        let detail: String = detail.chars().take(ERROR_CHARS).collect();
        bail!("{service} returned {status}: {detail}");
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chain_stops_at_first_success_and_keeps_earlier_failures() {
        let providers = ["first", "second", "third"];
        let calls = std::cell::Cell::new(0usize);
        let chain = run_chain(&providers, async |name: &&str| -> Result<usize> {
            calls.set(calls.get() + 1);
            if *name == "second" {
                Ok(name.len())
            } else {
                bail!("{name} is down")
            }
        })
        .await;
        assert_eq!(chain.answer, Some((1, 6)));
        assert_eq!(chain.failures, vec![(0, "first is down".to_string())]);
        assert_eq!(calls.get(), 2, "third provider must not be called");
    }

    #[tokio::test]
    async fn chain_reports_every_failure_when_none_answers() {
        let providers = ["a", "b"];
        let chain: Chain<()> = run_chain(&providers, async |name: &&str| -> Result<()> {
            bail!("{name} failed")
        })
        .await;
        assert!(chain.answer.is_none());
        assert_eq!(
            chain.failures,
            vec![(0, "a failed".to_string()), (1, "b failed".to_string())]
        );
    }

    #[test]
    fn credential_round_trips_through_the_sealer() {
        use base64::Engine as _;
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let sealer = Sealer::from_base64(&key).unwrap();
        let credential = Credential::ApiKey {
            api_key: "test-key".into(),
        };
        let sealed = seal_credential(&sealer, &credential).unwrap();
        let provider = EnabledProvider {
            id: 1,
            kind: KIND_ANTHROPIC_API.into(),
            model: "m".into(),
            sealed,
        };
        match open_credential(&sealer, &provider).unwrap() {
            Credential::ApiKey { api_key } => assert_eq!(api_key, "test-key"),
            Credential::OAuth { .. } => panic!("wrong credential variant"),
        }
    }
}
