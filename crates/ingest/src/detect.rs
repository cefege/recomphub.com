//! Platform detection from release asset names and AI-usage detection from repository
//! metadata (plan §5).

use recomphub_shared::{AiUsage, TargetPlatform};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// Case-insensitive substring rules, in enum order. `web` is never detected.
const PLATFORM_RULES: &[(TargetPlatform, &[&str])] = &[
    (
        TargetPlatform::Windows,
        &["windows", "-win", "_win", "win32", "win64", ".exe", ".msi"],
    ),
    (
        TargetPlatform::MacOs,
        &["mac", "macos", "osx", "darwin", ".dmg", "apple"],
    ),
    (
        TargetPlatform::Linux,
        &[
            "linux",
            ".appimage",
            ".flatpak",
            ".deb",
            ".rpm",
            "steamdeck",
        ],
    ),
    (TargetPlatform::Android, &[".apk", "android"]),
    (TargetPlatform::Ios, &[".ipa", "ios"]),
    (TargetPlatform::Switch, &[".nro", "switch"]),
    (TargetPlatform::PsVita, &[".vpk", "vita"]),
    (TargetPlatform::N3ds, &[".3dsx", ".cia"]),
];

/// Platforms whose release asset names match, unique, in enum order.
pub fn detect_platforms(asset_names: &[&str]) -> Vec<TargetPlatform> {
    let lowered: Vec<String> = asset_names.iter().map(|n| n.to_lowercase()).collect();
    PLATFORM_RULES
        .iter()
        .filter(|(_, patterns)| {
            lowered
                .iter()
                .any(|name| patterns.iter().any(|p| name.contains(*p)))
        })
        .map(|(platform, _)| *platform)
        .collect()
}

/// One piece of evidence for an AI detection. Stored as jsonb `[{signal, detail}]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiEvidence {
    pub signal: String,
    pub detail: String,
}

/// Result of [`detect_ai`]. `usage: None` means "no change to the stored value".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiDetection {
    pub usage: Option<AiUsage>,
    pub tools: Vec<String>,
    pub evidence: Vec<AiEvidence>,
}

/// A recent commit, used for co-author trailers and author logins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub message: String,
    pub author_login: Option<String>,
}

#[derive(Default)]
struct Signals {
    tool_hit: bool,
    human_hit: bool,
    tools: Vec<String>,
    evidence: Vec<AiEvidence>,
}

impl Signals {
    fn tool(&mut self, tool: &str, signal: &str, detail: String) {
        self.tool_hit = true;
        if !self.tools.iter().any(|t| t == tool) {
            self.tools.push(tool.to_string());
        }
        self.note(signal, detail);
    }

    fn human(&mut self, signal: &str, detail: String) {
        self.human_hit = true;
        self.note(signal, detail);
    }

    fn note(&mut self, signal: &str, detail: String) {
        if !self
            .evidence
            .iter()
            .any(|e| e.signal == signal && e.detail == detail)
        {
            self.evidence.push(AiEvidence {
                signal: signal.to_string(),
                detail,
            });
        }
    }
}

struct ReadmePatterns {
    /// (pattern, canonical tool, label used in evidence)
    tools: Vec<(Regex, &'static str, &'static str)>,
    human: Vec<Regex>,
}

fn readme_patterns() -> &'static ReadmePatterns {
    static PATTERNS: OnceLock<ReadmePatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let re = |p: &str| Regex::new(p).expect("static README regex compiles");
        ReadmePatterns {
            tools: vec![
                (re(r"(?i)\bclaude\b"), "Claude", "Claude"),
                (re(r"(?i)\bchatgpt\b"), "ChatGPT/Codex", "ChatGPT"),
                (re(r"(?i)\bcodex\b"), "ChatGPT/Codex", "Codex"),
                (re(r"(?i)\bcopilot\b"), "GitHub Copilot", "Copilot"),
                (re(r"(?i)\bgemini\b"), "Gemini", "Gemini"),
                (re(r"(?i)\bqwen\b"), "Qwen", "Qwen"),
                (re(r"(?i)\bdeepseek\b"), "DeepSeek", "DeepSeek"),
                (re(r"(?i)\bai[- ]assisted\b"), "Other", "AI-assisted phrase"),
                (
                    re(r"(?i)\bai[- ]generated\b"),
                    "Other",
                    "AI-generated phrase",
                ),
                (re(r"(?i)\bmade with ai\b"), "Other", "made-with-AI phrase"),
                (re(r"(?i)\bvibe[- ]coded\b"), "Other", "vibe-coded phrase"),
            ],
            human: vec![
                re(r"(?i)\b(no|without) (ai|llm)\b"),
                re(r"(?i)\bhuman[- ]made\b"),
            ],
        }
    })
}

/// Detects AI-assisted development from root file names, the Copilot instructions file,
/// the README, and recent commits. `root_files` are exact GitHub file names.
///
/// Outcome: tool signals only give `Assisted`; human signals only give `None`; both or
/// neither give `usage: None` (no change). Never returns `PrimarilyAi`.
pub fn detect_ai(
    root_files: &[&str],
    copilot_instructions_present: bool,
    readme: Option<&str>,
    commits: &[CommitInfo],
) -> AiDetection {
    let mut s = Signals::default();

    for file in root_files {
        let tool = match *file {
            "CLAUDE.md" | ".claude" => "Claude",
            "GEMINI.md" => "Gemini",
            ".cursorrules" | ".cursor" => "Cursor",
            ".windsurfrules" => "Windsurf",
            ".clinerules" => "Cline",
            "AGENTS.md" => "Other",
            _ => continue,
        };
        s.tool(tool, "root_file", format!("root file {file}"));
    }

    if copilot_instructions_present {
        s.tool(
            "GitHub Copilot",
            "copilot_instructions",
            ".github/copilot-instructions.md".to_string(),
        );
    }

    for commit in commits {
        let message = commit.message.to_lowercase();
        let author = commit.author_login.as_deref().map(str::to_lowercase);
        let author = author.as_deref().unwrap_or("");

        if message.contains("co-authored-by: claude")
            || message.contains("noreply@anthropic.com")
            || message.contains("generated with [claude code]")
        {
            s.tool(
                "Claude",
                "commit",
                "commit message trailer names Claude".to_string(),
            );
        }
        if author == "copilot-swe-agent" || message.contains("co-authored-by: copilot") {
            s.tool("GitHub Copilot", "commit", "commit by Copilot".to_string());
        }
        if author == "chatgpt-codex-connector" {
            s.tool(
                "ChatGPT/Codex",
                "commit",
                "commit by chatgpt-codex-connector".to_string(),
            );
        }
        if author == "cursoragent" || message.contains("co-authored-by: cursor") {
            s.tool("Cursor", "commit", "commit by Cursor agent".to_string());
        }
        if author == "google-labs-jules" {
            s.tool(
                "Gemini",
                "commit",
                "commit by google-labs-jules".to_string(),
            );
        }
    }

    if let Some(text) = readme {
        let patterns = readme_patterns();
        for (re, tool, label) in &patterns.tools {
            if re.is_match(text) {
                s.tool(tool, "readme", format!("README mentions {label}"));
            }
        }
        if patterns.human.iter().any(|re| re.is_match(text)) {
            s.human(
                "readme_human",
                "README says the project is human-made or without AI".to_string(),
            );
        }
    }

    let usage = match (s.tool_hit, s.human_hit) {
        (true, false) => Some(AiUsage::Assisted),
        (false, true) => Some(AiUsage::None),
        _ => None,
    };

    AiDetection {
        usage,
        tools: s.tools,
        evidence: s.evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recomphub_shared::AI_TOOLS;

    #[test]
    fn detects_platforms_from_release_assets() {
        let assets = [
            "Zelda64Recompiled-Windows.zip",
            "Zelda64Recompiled-Linux-x64.AppImage",
            "Zelda64Recompiled-macOS.dmg",
        ];
        assert_eq!(
            detect_platforms(&assets),
            vec![
                TargetPlatform::Windows,
                TargetPlatform::MacOs,
                TargetPlatform::Linux
            ]
        );
    }

    #[test]
    fn web_is_never_detected_and_no_match_is_empty() {
        assert!(detect_platforms(&["web-build.zip", "source.tar.gz"]).is_empty());
        assert!(detect_platforms(&[]).is_empty());
    }

    #[test]
    fn detects_mobile_and_handheld_assets() {
        assert_eq!(
            detect_platforms(&["app-release.apk", "game.vpk", "game.3dsx", "MyGame.nro"]),
            vec![
                TargetPlatform::Android,
                TargetPlatform::Switch,
                TargetPlatform::PsVita,
                TargetPlatform::N3ds,
            ]
        );
    }

    #[test]
    fn claude_md_root_file_is_assisted() {
        let d = detect_ai(&["README.md", "CLAUDE.md"], false, None, &[]);
        assert_eq!(d.usage, Some(AiUsage::Assisted));
        assert_eq!(d.tools, vec!["Claude".to_string()]);
        assert_eq!(d.evidence[0].signal, "root_file");
    }

    #[test]
    fn human_only_readme_gives_none() {
        let d = detect_ai(&[], false, Some("A fan project, made without AI."), &[]);
        assert_eq!(d.usage, Some(AiUsage::None));
        assert!(d.tools.is_empty());
    }

    #[test]
    fn mouse_cursor_in_readme_is_no_change() {
        let d = detect_ai(&[], false, Some("Move the mouse cursor to aim."), &[]);
        assert_eq!(d.usage, None);
        assert!(d.tools.is_empty());
        assert!(d.evidence.is_empty());
    }

    #[test]
    fn tool_and_human_signals_together_are_no_change() {
        let d = detect_ai(
            &[],
            false,
            Some("AI-assisted port, made without AI in the core."),
            &[],
        );
        assert_eq!(d.usage, None);
        assert_eq!(d.tools, vec!["Other".to_string()]);
    }

    #[test]
    fn readme_phrases_map_to_other_and_tool_names_word_bounded() {
        let d = detect_ai(
            &[],
            false,
            Some("This is a vibe-coded port. Uses Gemini."),
            &[],
        );
        assert_eq!(d.usage, Some(AiUsage::Assisted));
        assert_eq!(d.tools, vec!["Gemini".to_string(), "Other".to_string()]);
        let none = detect_ai(&[], false, Some("Claudette is a cat."), &[]);
        assert_eq!(none.usage, None);
    }

    #[test]
    fn commit_trailers_and_authors_are_detected() {
        let commits = [
            CommitInfo {
                message: "Fix\n\nCo-authored-by: Claude <noreply@anthropic.com>".into(),
                author_login: Some("mike".into()),
            },
            CommitInfo {
                message: "Bump".into(),
                author_login: Some("Copilot-SWE-Agent".into()),
            },
        ];
        let d = detect_ai(&[], false, None, &commits);
        assert_eq!(d.usage, Some(AiUsage::Assisted));
        assert_eq!(
            d.tools,
            vec!["Claude".to_string(), "GitHub Copilot".to_string()]
        );
    }

    #[test]
    fn copilot_file_and_root_names_are_exact() {
        let d = detect_ai(&["claude.md"], true, None, &[]);
        assert_eq!(d.tools, vec!["GitHub Copilot".to_string()]);
    }

    #[test]
    fn every_tool_name_is_canonical() {
        let readme = "Claude ChatGPT Codex Copilot Gemini Qwen DeepSeek AI-assisted";
        let d = detect_ai(&[], false, Some(readme), &[]);
        for tool in &d.tools {
            assert!(AI_TOOLS.contains(&tool.as_str()), "{tool} is not canonical");
        }
    }

    #[test]
    fn evidence_serializes_as_signal_detail_objects() {
        let d = detect_ai(&["AGENTS.md"], false, None, &[]);
        let v = serde_json::to_value(&d.evidence).unwrap();
        assert_eq!(v[0]["signal"], "root_file");
        assert_eq!(v[0]["detail"], "root file AGENTS.md");
    }
}
