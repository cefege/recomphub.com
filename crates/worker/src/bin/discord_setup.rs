//! Idempotently configures the RecompHub Discord community through Discord's bot API.

use std::{collections::HashSet, env, process::ExitCode, time::Duration};

use base64::Engine;

use anyhow::{Context, Result, bail};
use clap::Parser;
use reqwest::{Client, RequestBuilder};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

const API: &str = "https://discord.com/api/v10";
const GUILD_ID: &str = "1561674685316472994";
const SEND_MESSAGES: u64 = 1 << 11;
const CREATE_PUBLIC_THREADS: u64 = 1 << 35;
const SEND_MESSAGES_IN_THREADS: u64 = 1 << 38;
const CATEGORY_NAME: &str = "COMMUNITY";
/// Website CTA placements that get their own invite under `--placement-invites`.
const INVITE_PLACEMENTS: &[&str] = &["header", "footer", "home", "listing"];
const SERVER_ICON_PNG: &[u8] = include_bytes!("../../../web/assets/recomphub-logo.png");

struct ChannelSpec {
    name: &'static str,
    topic: &'static str,
    read_only: bool,
    /// Members can't post top-level here, but can create and reply in threads under each
    /// message, so discord-announce's posts stay repliable without cluttering the channel.
    allow_threads: bool,
}

const CHANNELS: &[ChannelSpec] = &[
    ChannelSpec {
        name: "welcome",
        topic: "Start here: be kind, do not share copyrighted game files or links, then say hello in #general.",
        read_only: true,
        allow_threads: false,
    },
    ChannelSpec {
        name: "announcements",
        topic: "Project news, releases, and events. Reply in a message's thread to discuss it.",
        read_only: true,
        allow_threads: true,
    },
    ChannelSpec {
        name: "general",
        topic: "Ask questions, coordinate, and talk about recompiled and decompiled ports.",
        read_only: false,
        allow_threads: false,
    },
    ChannelSpec {
        name: "showcase",
        topic: "Share ports, builds, progress, screenshots, demos, and useful links. One project per post.",
        read_only: false,
        allow_threads: false,
    },
    ChannelSpec {
        name: "website-bugs",
        topic: "Report recomphub.com bugs. Include the page URL, expected and actual results, browser/device, and screenshots or console errors.",
        read_only: false,
        allow_threads: false,
    },
];

#[derive(Debug, Parser)]
#[command(
    name = "discord-setup",
    about = "Configure the RecompHub Discord community"
)]
struct Cli {
    /// Print intended mutations without writing to Discord.
    #[arg(long)]
    dry_run: bool,

    /// Delete channels and categories outside the lean community layout.
    #[arg(long)]
    prune: bool,

    /// Replace the Discord server icon with the bundled RecompHub logo.
    #[arg(long)]
    server_icon: bool,

    /// Mint one additional permanent, unique invite per website CTA placement (header, footer,
    /// home, listing), so each shows its own use count under Server Settings -> Invites. Run
    /// once when wiring up a new placement, then hardcode the printed codes into `app.rs`:
    /// re-running always mints fresh codes instead of reusing them.
    #[arg(long)]
    placement_invites: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Channel {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: u8,
    parent_id: Option<String>,
    #[serde(default)]
    topic: Option<String>,
}

fn planned_channel(spec: &ChannelSpec, category: &Channel) -> Channel {
    Channel {
        id: format!("<new {}>", spec.name),
        name: spec.name.to_string(),
        kind: 0,
        parent_id: Some(category.id.clone()),
        topic: Some(spec.topic.to_string()),
    }
}

fn desired_channels(channels: &[Channel]) -> Result<Vec<Channel>> {
    CHANNELS
        .iter()
        .map(|spec| {
            channels
                .iter()
                .find(|channel| channel.kind == 0 && channel.name == spec.name)
                .cloned()
                .with_context(|| format!("#{} is missing", spec.name))
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct Invite {
    code: String,
}

struct Discord {
    http: Client,
    token: String,
}

impl Discord {
    fn from_env() -> Result<Self> {
        let token = env::var("DISCORD_BOT_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .context("DISCORD_BOT_TOKEN is not set; add it to .env")?;
        let http = Client::builder()
            .user_agent("RecompHub Discord setup/1.0")
            .timeout(Duration::from_secs(30))
            .build()
            .context("building Discord HTTP client")?;
        Ok(Self { http, token })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{API}{path}"))
            .header("Authorization", format!("Bot {}", self.token))
    }

    async fn json<T: DeserializeOwned>(&self, request: RequestBuilder, action: &str) -> Result<T> {
        let response = request.send().await.with_context(|| action.to_owned())?;
        let status = response.status();
        let body = response.text().await.context("reading Discord response")?;
        if !status.is_success() {
            bail!("{action} failed ({status}): {body}");
        }
        serde_json::from_str(&body).with_context(|| format!("decoding {action} response"))
    }

    async fn empty(&self, request: RequestBuilder, action: &str) -> Result<()> {
        let response = request.send().await.with_context(|| action.to_owned())?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("{action} failed ({status}): {body}");
        }
        Ok(())
    }

    async fn channels(&self) -> Result<Vec<Channel>> {
        self.json(
            self.request(
                reqwest::Method::GET,
                &format!("/guilds/{GUILD_ID}/channels"),
            ),
            "listing channels",
        )
        .await
    }

    async fn create_channel(
        &self,
        name: &str,
        kind: u8,
        parent_id: Option<&str>,
        topic: Option<&str>,
    ) -> Result<Channel> {
        let body = match topic {
            Some(topic) => {
                json!({"name": name, "type": kind, "parent_id": parent_id, "topic": topic})
            }
            None => json!({"name": name, "type": kind, "parent_id": parent_id}),
        };
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/guilds/{GUILD_ID}/channels"),
            )
            .json(&body),
            &format!("creating {name}"),
        )
        .await
    }

    async fn update_channel(&self, channel: &Channel, parent_id: &str, topic: &str) -> Result<()> {
        self.empty(
            self.request(reqwest::Method::PATCH, &format!("/channels/{}", channel.id))
                .json(&json!({"parent_id": parent_id, "topic": topic})),
            &format!("updating #{}", channel.name),
        )
        .await
    }

    async fn order_channels(&self, channels: &[Channel]) -> Result<()> {
        let positions: Vec<_> = channels
            .iter()
            .enumerate()
            .map(|(position, channel)| json!({"id": channel.id, "position": position}))
            .collect();
        self.empty(
            self.request(
                reqwest::Method::PATCH,
                &format!("/guilds/{GUILD_ID}/channels"),
            )
            .json(&positions),
            "ordering channels",
        )
        .await
    }

    async fn delete_channel(&self, channel: &Channel) -> Result<()> {
        self.empty(
            self.request(
                reqwest::Method::DELETE,
                &format!("/channels/{}", channel.id),
            ),
            &format!("deleting {}", channel.name),
        )
        .await
    }

    async fn overwrite(
        &self,
        channel_id: &str,
        subject_id: &str,
        allow: u64,
        deny: u64,
    ) -> Result<()> {
        self.empty(
            self.request(
                reqwest::Method::PUT,
                &format!("/channels/{channel_id}/permissions/{subject_id}"),
            )
            .json(&json!({
                "id": subject_id,
                "type": 0,
                "allow": allow.to_string(),
                "deny": deny.to_string()
            })),
            "updating channel permissions",
        )
        .await
    }
    async fn clear_overwrite(&self, channel_id: &str, subject_id: &str) -> Result<()> {
        self.empty(
            self.request(
                reqwest::Method::DELETE,
                &format!("/channels/{channel_id}/permissions/{subject_id}"),
            ),
            "clearing channel permissions",
        )
        .await
    }

    async fn set_server_icon(&self, icon: &[u8]) -> Result<()> {
        let icon = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(icon)
        );
        self.empty(
            self.request(reqwest::Method::PATCH, &format!("/guilds/{GUILD_ID}"))
                .json(&json!({"icon": icon})),
            "updating server icon",
        )
        .await
    }

    async fn invite(&self, channel_id: &str) -> Result<Invite> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/channels/{channel_id}/invites"),
            )
            .json(&json!({"max_age": 0, "max_uses": 0, "temporary": false, "unique": false})),
            "creating permanent invite",
        )
        .await
    }

    /// Always mints a fresh invite, unlike `invite` which dedupes to an existing one with the
    /// same settings. Used for per-placement invites, where each needs its own code.
    async fn unique_invite(&self, channel_id: &str) -> Result<Invite> {
        self.json(
            self.request(
                reqwest::Method::POST,
                &format!("/channels/{channel_id}/invites"),
            )
            .json(&json!({"max_age": 0, "max_uses": 0, "temporary": false, "unique": true})),
            "creating placement invite",
        )
        .await
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("discord-setup: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let discord = Discord::from_env()?;
    if cli.server_icon {
        if cli.dry_run {
            println!("UPDATE RecompHub server icon");
        } else {
            discord.set_server_icon(SERVER_ICON_PNG).await?;
            println!("UPDATED RecompHub server icon");
        }
    }
    let mut channels = discord.channels().await?;
    let mut category_matches: Vec<_> = channels
        .iter()
        .filter(|channel| channel.kind == 4 && channel.name == CATEGORY_NAME)
        .cloned()
        .collect();
    category_matches.sort_by(|a, b| a.id.cmp(&b.id));
    let category = match category_matches.first() {
        Some(category) => category.clone(),
        None if cli.dry_run => {
            println!("CREATE category {CATEGORY_NAME}");
            Channel {
                id: format!("<new {CATEGORY_NAME} category>"),
                name: CATEGORY_NAME.to_string(),
                kind: 4,
                parent_id: None,
                topic: None,
            }
        }
        None => {
            let category = discord.create_channel(CATEGORY_NAME, 4, None, None).await?;
            println!("CREATED category {CATEGORY_NAME} ({})", category.id);
            channels.push(category.clone());
            category
        }
    };
    for duplicate in category_matches.iter().skip(1) {
        if cli.dry_run {
            println!("DELETE duplicate category {}", duplicate.name);
        } else {
            discord.delete_channel(duplicate).await?;
            println!("DELETED duplicate category {}", duplicate.name);
            channels.retain(|channel| channel.id != duplicate.id);
        }
    }

    let mut desired_ids = HashSet::new();
    for spec in CHANNELS {
        let mut matches: Vec<_> = channels
            .iter()
            .filter(|channel| channel.kind == 0 && channel.name == spec.name)
            .cloned()
            .collect();
        matches.sort_by(|a, b| a.id.cmp(&b.id));
        let channel = match matches.first() {
            Some(channel) => {
                if channel.parent_id.as_deref() != Some(&category.id)
                    || channel.topic.as_deref() != Some(spec.topic)
                {
                    if cli.dry_run {
                        println!("UPDATE #{} in {CATEGORY_NAME}", spec.name);
                    } else {
                        discord
                            .update_channel(channel, &category.id, spec.topic)
                            .await?;
                        println!("UPDATED #{} in {CATEGORY_NAME}", spec.name);
                    }
                }
                channel.clone()
            }
            None if cli.dry_run => {
                println!("CREATE #{} in {CATEGORY_NAME}", spec.name);
                let channel = planned_channel(spec, &category);
                channels.push(channel.clone());
                channel
            }
            None => {
                let channel = discord
                    .create_channel(spec.name, 0, Some(&category.id), Some(spec.topic))
                    .await?;
                println!("CREATED #{} ({})", spec.name, channel.id);
                channels.push(channel.clone());
                channel
            }
        };
        desired_ids.insert(channel.id.clone());
        for duplicate in matches.iter().skip(1) {
            if cli.dry_run {
                println!("DELETE duplicate #{}", duplicate.name);
            } else {
                discord.delete_channel(duplicate).await?;
                println!("DELETED duplicate #{}", duplicate.name);
                channels.retain(|channel| channel.id != duplicate.id);
            }
        }
    }

    if cli.prune {
        let unwanted_channels: Vec<_> = channels
            .iter()
            .filter(|channel| channel.kind != 4 && !desired_ids.contains(&channel.id))
            .cloned()
            .collect();
        for channel in unwanted_channels {
            if cli.dry_run {
                println!("DELETE #{}", channel.name);
            } else {
                discord.delete_channel(&channel).await?;
                println!("DELETED #{}", channel.name);
                channels.retain(|candidate| candidate.id != channel.id);
            }
        }
        let unwanted_categories: Vec<_> = channels
            .iter()
            .filter(|channel| channel.kind == 4 && channel.id != category.id)
            .cloned()
            .collect();
        for channel in unwanted_categories {
            if cli.dry_run {
                println!("DELETE category {}", channel.name);
            } else {
                discord.delete_channel(&channel).await?;
                println!("DELETED category {}", channel.name);
                channels.retain(|candidate| candidate.id != channel.id);
            }
        }
    }

    let ordered_channels = desired_channels(&channels)?;
    if cli.dry_run {
        println!("ORDER #welcome, #announcements, #general, #showcase, #website-bugs");
    } else {
        discord.order_channels(&ordered_channels).await?;
    }

    for (spec, channel) in CHANNELS.iter().zip(&ordered_channels) {
        if spec.read_only {
            let allow = if spec.allow_threads {
                CREATE_PUBLIC_THREADS | SEND_MESSAGES_IN_THREADS
            } else {
                0
            };
            if cli.dry_run {
                if spec.allow_threads {
                    println!(
                        "DENY SEND_MESSAGES, ALLOW thread replies to @everyone in #{}",
                        spec.name
                    );
                } else {
                    println!("DENY SEND_MESSAGES to @everyone in #{}", spec.name);
                }
            } else {
                discord
                    .overwrite(&channel.id, GUILD_ID, allow, SEND_MESSAGES)
                    .await?;
            }
        } else if cli.dry_run {
            println!("CLEAR @everyone override in #{}", spec.name);
        } else {
            discord.clear_overwrite(&channel.id, GUILD_ID).await?;
        }
    }

    if cli.dry_run {
        println!("CREATE permanent invite from #welcome");
        if cli.placement_invites {
            for name in INVITE_PLACEMENTS {
                println!("CREATE placement invite [{name}] from #welcome");
            }
        }
    } else {
        let welcome = channels
            .iter()
            .find(|channel| channel.kind == 0 && channel.name == "welcome")
            .context("#welcome is missing")?;
        let invite = discord.invite(&welcome.id).await?;
        println!("INVITE https://discord.gg/{}", invite.code);
        if cli.placement_invites {
            for name in INVITE_PLACEMENTS {
                let invite = discord.unique_invite(&welcome.id).await?;
                println!("INVITE[{name}] https://discord.gg/{}", invite.code);
            }
        }
    }

    let final_channels = discord.channels().await?;
    let category = final_channels
        .iter()
        .find(|channel| channel.kind == 4 && channel.name == CATEGORY_NAME);
    println!("\nFinal layout:");
    println!(
        "{CATEGORY_NAME} ({})",
        category.map_or("missing", |channel| channel.id.as_str())
    );
    for spec in CHANNELS {
        let channel = final_channels
            .iter()
            .find(|channel| channel.kind == 0 && channel.name == spec.name);
        println!(
            "  #{} ({}) — {}",
            spec.name,
            channel.map_or("missing", |channel| channel.id.as_str()),
            spec.topic
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_missing_channels_feed_downstream_planning() {
        let category = Channel {
            id: "<new COMMUNITY category>".to_string(),
            name: CATEGORY_NAME.to_string(),
            kind: 4,
            parent_id: None,
            topic: None,
        };
        let mut channels = Vec::new();
        for spec in CHANNELS {
            channels.push(planned_channel(spec, &category));
        }

        let planned = desired_channels(&channels).expect("planned channels are available");

        assert_eq!(planned.len(), CHANNELS.len());
        assert!(
            planned
                .iter()
                .all(|channel| channel.parent_id.as_deref() == Some(category.id.as_str()))
        );
        assert_eq!(
            planned
                .iter()
                .map(|channel| channel.name.as_str())
                .collect::<Vec<_>>(),
            [
                "welcome",
                "announcements",
                "general",
                "showcase",
                "website-bugs"
            ]
        );
    }
}
