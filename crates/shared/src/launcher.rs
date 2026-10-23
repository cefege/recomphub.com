//! Reviewed desktop install metadata and host-matching rules.
//!
//! This module deliberately contains only serde-compatible DTOs and pure validation so it can
//! cross the server, native client, and wasm boundaries without importing platform APIs.

use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fmt;
use url::Url;

use crate::{GameCard, PortCard};

pub const INSTALL_MANIFEST_SCHEMA_VERSION: u32 = 1;
const MAX_ID_LEN: usize = 120;
const MAX_PATH_LEN: usize = 512;
const MAX_TEXT_LEN: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostOs {
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuArch {
    Aarch64,
    X86_64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageFormat {
    Zip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPlatform {
    pub os: HostOs,
    pub arch: CpuArch,
    /// Numeric OS version, e.g. `13.0` or `10.0.22631`.
    pub os_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpec {
    /// Package-relative path to the executable. Never a shell command.
    pub executable: String,
    /// Package-relative working directory.
    pub working_directory: String,
    #[serde(default)]
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecipe {
    pub id: String,
    pub revision: u32,
    pub repo_url: String,
    pub os: HostOs,
    pub architectures: Vec<CpuArch>,
    pub minimum_os_version: Option<String>,
    /// Literal `{tag}` substitution only.
    pub asset_name_template: String,
    pub package_format: PackageFormat,
    pub launch: LaunchSpec,
    pub data_adapter: Option<String>,
    #[serde(default)]
    pub requirements: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallManifest {
    pub schema_version: u32,
    pub recipe: InstallRecipe,
    pub port_id: i64,
    pub game_id: i64,
    pub release_id: i64,
    pub asset_id: i64,
    /// The upstream tag, retained verbatim for URL and asset-template matching.
    pub tag: String,
    /// Stable semantic version with a leading `v` removed.
    pub version: String,
    pub published_at: DateTime<Utc>,
    pub release_url: String,
    pub asset_name: String,
    pub download_url: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstallOffer {
    pub game: GameCard,
    pub port: PortCard,
    pub manifest: InstallManifest,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallOffersQuery {
    pub host: HostPlatform,
    pub port_id: Option<i64>,
    pub page: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherValidationError(String);

impl LauncherValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for LauncherValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LauncherValidationError {}

impl HostPlatform {
    pub fn validate(&self) -> Result<(), LauncherValidationError> {
        parse_numeric_version(&self.os_version).map(|_| ())
    }
}

/// A strict GitHub repository identity. Original spelling is retained for release URLs while
/// owner/repository comparisons are ASCII case-insensitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRepository {
    pub owner: String,
    pub repository: String,
}

impl GithubRepository {
    pub fn matches(&self, other: &Self) -> bool {
        self.owner.eq_ignore_ascii_case(&other.owner)
            && self.repository.eq_ignore_ascii_case(&other.repository)
    }
}

/// Parses a canonical HTTPS GitHub repository URL with exactly owner and repository segments.
pub fn parse_github_repository_url(
    value: &str,
) -> Result<GithubRepository, LauncherValidationError> {
    let url = parse_github_url("recipe repository", value)?;
    let segments = exact_path_segments("recipe repository", &url)?;
    if segments.len() != 2
        || !valid_github_owner(segments[0])
        || !valid_github_repository(segments[1])
    {
        return Err(LauncherValidationError::new(
            "recipe repository must identify exactly one GitHub owner and repository",
        ));
    }
    Ok(GithubRepository {
        owner: segments[0].to_owned(),
        repository: segments[1].to_owned(),
    })
}

/// Validates a release download URL against its reviewed repository. Exact tag and asset matching
/// belongs to [`validate_github_release_urls`].
pub fn validate_github_release_download_url(
    repo_url: &str,
    download_url: &str,
) -> Result<(), LauncherValidationError> {
    let repo = parse_github_repository_url(repo_url)?;
    let url = parse_github_url("download URL", download_url)?;
    let segments = exact_path_segments("download URL", &url)?;
    if segments.len() != 6
        || !repo.owner.eq_ignore_ascii_case(segments[0])
        || !repo.repository.eq_ignore_ascii_case(segments[1])
        || segments[2] != "releases"
        || segments[3] != "download"
    {
        return Err(LauncherValidationError::new(
            "download URL must identify the reviewed GitHub release asset",
        ));
    }
    validate_url_component("download tag", segments[4], MAX_ID_LEN)?;
    validate_url_component("download asset name", segments[5], MAX_PATH_LEN)
}

/// Validates the exact release and download URLs embedded in a manifest.
pub fn validate_github_release_urls(
    repo_url: &str,
    tag: &str,
    asset_name: &str,
    release_url: &str,
    download_url: &str,
) -> Result<(), LauncherValidationError> {
    let repo = parse_github_repository_url(repo_url)?;
    validate_url_component("release tag", tag, MAX_ID_LEN)?;
    validate_url_component("asset name", asset_name, MAX_PATH_LEN)?;

    let release = parse_github_url("release URL", release_url)?;
    let segments = exact_path_segments("release URL", &release)?;
    if segments.len() != 5
        || !repo.owner.eq_ignore_ascii_case(segments[0])
        || !repo.repository.eq_ignore_ascii_case(segments[1])
        || segments[2] != "releases"
        || segments[3] != "tag"
        || segments[4] != tag
    {
        return Err(LauncherValidationError::new(
            "release URL must exactly match the reviewed repository and tag",
        ));
    }

    validate_github_release_download_url(repo_url, download_url)?;
    let download = parse_github_url("download URL", download_url)?;
    let segments = exact_path_segments("download URL", &download)?;
    if segments[4] != tag || segments[5] != asset_name {
        return Err(LauncherValidationError::new(
            "download URL must exactly match the manifest tag and asset name",
        ));
    }
    Ok(())
}

impl InstallRecipe {
    pub fn validate(&self) -> Result<(), LauncherValidationError> {
        bounded("recipe id", &self.id, MAX_ID_LEN)?;
        if self.revision == 0 {
            return Err(LauncherValidationError::new(
                "recipe revision must be positive",
            ));
        }
        parse_github_repository_url(&self.repo_url)?;
        if self.architectures.is_empty() {
            return Err(LauncherValidationError::new(
                "recipe must include an architecture",
            ));
        }
        if self.architectures.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(LauncherValidationError::new(
                "recipe architectures must be unique",
            ));
        }
        if let Some(version) = &self.minimum_os_version {
            parse_numeric_version(version)?;
        }
        validate_asset_name_template(&self.asset_name_template)?;
        validate_package_relative_path("executable", &self.launch.executable, false)?;
        validate_package_relative_path("working directory", &self.launch.working_directory, true)?;
        if self.launch.arguments.len() > 32 {
            return Err(LauncherValidationError::new(
                "launch arguments must contain at most 32 entries",
            ));
        }
        for argument in &self.launch.arguments {
            bounded_argument(argument)?;
        }
        if let Some(adapter) = &self.data_adapter {
            bounded("data adapter", adapter, MAX_ID_LEN)?;
        }
        if self.requirements.len() > 32 {
            return Err(LauncherValidationError::new(
                "requirements must contain at most 32 entries",
            ));
        }
        for requirement in &self.requirements {
            bounded("requirement", requirement, MAX_TEXT_LEN)?;
        }
        Ok(())
    }

    pub fn expected_asset_name(&self, tag: &str) -> Result<String, LauncherValidationError> {
        validate_url_component("release tag", tag, MAX_ID_LEN)?;
        let asset_name = self.asset_name_template.replace("{tag}", tag);
        validate_url_component("asset name", &asset_name, MAX_PATH_LEN)?;
        Ok(asset_name)
    }

    pub fn matches_host(&self, host: &HostPlatform) -> Result<bool, LauncherValidationError> {
        self.validate()?;
        host.validate()?;
        if self.os != host.os || !self.architectures.contains(&host.arch) {
            return Ok(false);
        }
        self.minimum_os_version
            .as_deref()
            .map(|minimum| numeric_version_at_least(&host.os_version, minimum))
            .transpose()
            .map(|matches| matches.unwrap_or(true))
    }
}

impl InstallManifest {
    pub fn validate(&self) -> Result<(), LauncherValidationError> {
        if self.schema_version != INSTALL_MANIFEST_SCHEMA_VERSION {
            return Err(LauncherValidationError::new(
                "unsupported install manifest schema",
            ));
        }
        self.recipe.validate()?;
        for (name, value) in [
            ("port id", self.port_id),
            ("game id", self.game_id),
            ("release id", self.release_id),
            ("asset id", self.asset_id),
        ] {
            if value <= 0 {
                return Err(LauncherValidationError::new(format!(
                    "{name} must be positive"
                )));
            }
        }
        validate_url_component("release tag", &self.tag, MAX_ID_LEN)?;
        let version = self.version.strip_prefix('v').unwrap_or(&self.version);
        if version != self.version || !is_stable_semver(version) {
            return Err(LauncherValidationError::new(
                "manifest version must be stable semantic version without v",
            ));
        }
        if self.tag.strip_prefix('v').unwrap_or(&self.tag) != self.version {
            return Err(LauncherValidationError::new(
                "manifest version does not match release tag",
            ));
        }
        if self.asset_name != self.recipe.expected_asset_name(&self.tag)? {
            return Err(LauncherValidationError::new(
                "manifest asset does not match recipe template",
            ));
        }
        validate_github_release_urls(
            &self.recipe.repo_url,
            &self.tag,
            &self.asset_name,
            &self.release_url,
            &self.download_url,
        )?;
        if self.size_bytes == 0 || self.size_bytes > 512 * 1024 * 1024 {
            return Err(LauncherValidationError::new(
                "asset size must be between 1 byte and 512 MiB",
            ));
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(LauncherValidationError::new(
                "manifest SHA-256 must be 64 hexadecimal characters",
            ));
        }
        Ok(())
    }

    pub fn matches_host(&self, host: &HostPlatform) -> Result<bool, LauncherValidationError> {
        self.validate()?;
        self.recipe.matches_host(host)
    }
}

impl InstallOffer {
    pub fn validate(&self) -> Result<(), LauncherValidationError> {
        self.manifest.validate()?;
        if self.game.id != self.manifest.game_id || self.port.id != self.manifest.port_id {
            return Err(LauncherValidationError::new(
                "offer cards do not agree with manifest IDs",
            ));
        }
        if self.port.game_id != self.game.id || self.port.game_slug != self.game.slug {
            return Err(LauncherValidationError::new(
                "offer port does not agree with offer game",
            ));
        }
        bounded("game slug", &self.game.slug, MAX_ID_LEN)?;
        bounded("game name", &self.game.name, MAX_TEXT_LEN)?;
        bounded("port slug", &self.port.slug, MAX_ID_LEN)?;
        bounded("port game slug", &self.port.game_slug, MAX_ID_LEN)?;
        bounded("port name", &self.port.name, MAX_TEXT_LEN)?;
        if !parse_github_repository_url(&self.port.repo_url)?.matches(&parse_github_repository_url(
            &self.manifest.recipe.repo_url,
        )?) {
            return Err(LauncherValidationError::new(
                "offer port repository does not agree with manifest recipe",
            ));
        }
        Ok(())
    }
}

pub fn parse_numeric_version(value: &str) -> Result<Vec<u64>, LauncherValidationError> {
    if value.is_empty() || value.len() > 64 {
        return Err(LauncherValidationError::new(
            "OS version is missing or too long",
        ));
    }
    value
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(LauncherValidationError::new(
                    "OS version must contain numeric components",
                ));
            }
            part.parse::<u64>()
                .map_err(|_| LauncherValidationError::new("OS version component is out of range"))
        })
        .collect()
}

pub fn numeric_version_at_least(
    actual: &str,
    minimum: &str,
) -> Result<bool, LauncherValidationError> {
    let actual = parse_numeric_version(actual)?;
    let minimum = parse_numeric_version(minimum)?;
    let components = actual.len().max(minimum.len());
    Ok((0..components)
        .find_map(|index| {
            let a = actual.get(index).copied().unwrap_or(0);
            let b = minimum.get(index).copied().unwrap_or(0);
            (a != b).then_some(a > b)
        })
        .unwrap_or(true))
}

fn is_stable_semver(value: &str) -> bool {
    value
        .parse::<Version>()
        .is_ok_and(|version| version.pre.is_empty() && version.build.is_empty())
}

fn bounded(name: &str, value: &str, maximum: usize) -> Result<(), LauncherValidationError> {
    if value.trim().is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(LauncherValidationError::new(format!(
            "{name} is missing or too long"
        )));
    }
    Ok(())
}

fn bounded_argument(value: &str) -> Result<(), LauncherValidationError> {
    if value.len() > MAX_TEXT_LEN || value.contains('\0') {
        return Err(LauncherValidationError::new(
            "launch argument contains NUL or is too long",
        ));
    }
    Ok(())
}

/// Validates package-relative paths identically on every host. Native canonical containment is an
/// additional check at filesystem mutation time.
pub fn validate_package_relative_path(
    name: &str,
    value: &str,
    allow_current_directory: bool,
) -> Result<(), LauncherValidationError> {
    if allow_current_directory && value == "." {
        return Ok(());
    }
    bounded(name, value, MAX_PATH_LEN)?;
    if value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.contains(':')
        || value
            .split('/')
            .any(|component| !safe_package_component(component))
    {
        return Err(LauncherValidationError::new(format!(
            "{name} must be a safe package-relative path"
        )));
    }
    Ok(())
}

fn validate_asset_name_template(value: &str) -> Result<(), LauncherValidationError> {
    bounded("asset name template", value, MAX_PATH_LEN)?;
    if value.matches("{tag}").count() != 1 {
        return Err(LauncherValidationError::new(
            "asset name template must contain exactly one {tag}",
        ));
    }
    validate_url_component(
        "asset name template",
        &value.replace("{tag}", "tag"),
        MAX_PATH_LEN,
    )
}

fn validate_url_component(
    name: &str,
    value: &str,
    maximum: usize,
) -> Result<(), LauncherValidationError> {
    bounded(name, value, maximum)?;
    if !safe_url_component(value) {
        return Err(LauncherValidationError::new(format!(
            "{name} must be one safe URL path component"
        )));
    }
    Ok(())
}

fn safe_package_component(component: &str) -> bool {
    safe_url_component(component)
        && !component.contains(':')
        && !component
            .as_bytes()
            .last()
            .is_some_and(|byte| matches!(byte, b'.' | b' '))
        && !component
            .chars()
            .any(|character| matches!(character, '*' | '?' | '"' | '<' | '>' | '|'))
        && !is_windows_reserved_component(component)
}

fn safe_url_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component
            .chars()
            .any(|character| matches!(character, '/' | '\\' | '%' | '\0') || character.is_control())
}

/// Validates one package path component using Windows restrictions on every host.
pub fn validate_package_path_component(
    name: &str,
    component: &str,
) -> Result<(), LauncherValidationError> {
    if !safe_package_component(component) {
        return Err(LauncherValidationError::new(format!(
            "{name} contains an unsafe package path component"
        )));
    }
    Ok(())
}

fn is_windows_reserved_component(component: &str) -> bool {
    let basename = component
        .split('.')
        .next()
        .unwrap_or_default()
        .to_uppercase();
    matches!(basename.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || basename
            .strip_prefix("COM")
            .or_else(|| basename.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(
                    number,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
}

fn parse_github_url(name: &str, value: &str) -> Result<Url, LauncherValidationError> {
    bounded(name, value, MAX_TEXT_LEN)?;
    let url = Url::parse(value)
        .map_err(|_| LauncherValidationError::new(format!("{name} must be an HTTPS URL")))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || !url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(LauncherValidationError::new(format!(
            "{name} must be an HTTPS GitHub URL without credentials, query, or fragment"
        )));
    }
    Ok(url)
}

fn exact_path_segments<'a>(
    name: &str,
    url: &'a Url,
) -> Result<Vec<&'a str>, LauncherValidationError> {
    let path = url.path();
    if !path.starts_with('/') || path.contains('%') {
        return Err(LauncherValidationError::new(format!(
            "{name} contains an encoded separator or invalid path"
        )));
    }
    let segments: Vec<_> = path[1..].split('/').collect();
    if segments.iter().any(|segment| !safe_url_component(segment)) {
        return Err(LauncherValidationError::new(format!(
            "{name} contains an invalid path component"
        )));
    }
    Ok(segments)
}

fn valid_github_owner(value: &str) -> bool {
    value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn valid_github_repository(value: &str) -> bool {
    value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> InstallRecipe {
        InstallRecipe {
            id: "banjo-recompiled-macos".into(),
            revision: 1,
            repo_url: "https://github.com/banjorecomp/banjorecomp".into(),
            os: HostOs::Macos,
            architectures: vec![CpuArch::Aarch64, CpuArch::X86_64],
            minimum_os_version: Some("13.0".into()),
            asset_name_template: "BanjoRecompiled-{tag}-macOS.zip".into(),
            package_format: PackageFormat::Zip,
            launch: LaunchSpec {
                executable: "BanjoRecompiled.app/Contents/MacOS/BanjoRecompiled".into(),
                working_directory: "BanjoRecompiled.app/Contents/Resources".into(),
                arguments: vec![],
            },
            data_adapter: Some("banjo_n64_us_10".into()),
            requirements: vec!["North American 1.0 ROM".into()],
        }
    }

    fn manifest() -> InstallManifest {
        InstallManifest {
            schema_version: INSTALL_MANIFEST_SCHEMA_VERSION,
            recipe: recipe(),
            port_id: 1,
            game_id: 2,
            release_id: 3,
            asset_id: 4,
            tag: "v1.0.2".into(),
            version: "1.0.2".into(),
            published_at: Utc::now(),
            release_url: "https://github.com/BanjoRecomp/BanjoRecomp/releases/tag/v1.0.2".into(),
            asset_name: "BanjoRecompiled-v1.0.2-macOS.zip".into(),
            download_url: "https://github.com/BanjoRecomp/BanjoRecomp/releases/download/v1.0.2/BanjoRecompiled-v1.0.2-macOS.zip".into(),
            size_bytes: 1,
            sha256: "ab".repeat(32),
        }
    }

    #[test]
    fn host_matching_requires_native_architecture_and_numeric_version() {
        let recipe = recipe();
        assert!(
            recipe
                .matches_host(&HostPlatform {
                    os: HostOs::Macos,
                    arch: CpuArch::Aarch64,
                    os_version: "13".into()
                })
                .unwrap()
        );
        assert!(
            !recipe
                .matches_host(&HostPlatform {
                    os: HostOs::Macos,
                    arch: CpuArch::Aarch64,
                    os_version: "12.6".into()
                })
                .unwrap()
        );
        assert!(
            !recipe
                .matches_host(&HostPlatform {
                    os: HostOs::Windows,
                    arch: CpuArch::Aarch64,
                    os_version: "11".into()
                })
                .unwrap()
        );
        assert!(
            recipe
                .matches_host(&HostPlatform {
                    os: HostOs::Macos,
                    arch: CpuArch::Aarch64,
                    os_version: "thirteen".into()
                })
                .is_err()
        );
    }

    #[test]
    fn numeric_versions_pad_omitted_components() {
        assert!(numeric_version_at_least("13", "13.0.0").unwrap());
        assert!(numeric_version_at_least("13.0.1", "13").unwrap());
        assert!(!numeric_version_at_least("12.10", "13").unwrap());
    }

    #[test]
    fn recipe_rejects_unsafe_launch_paths() {
        let mut recipe = recipe();
        recipe.launch.executable = "../BanjoRecompiled".into();
        assert!(recipe.validate().is_err());
        recipe.launch.executable = "BanjoRecompiled.app/Contents/MacOS/BanjoRecompiled".into();
        recipe.launch.working_directory = "/tmp".into();
        assert!(recipe.validate().is_err());
    }

    #[test]
    fn manifest_accepts_mixed_case_github_urls_without_normalizing_them() {
        let manifest = manifest();
        assert!(manifest.validate().is_ok());
        assert_eq!(
            manifest.download_url,
            "https://github.com/BanjoRecomp/BanjoRecomp/releases/download/v1.0.2/BanjoRecompiled-v1.0.2-macOS.zip"
        );
    }

    #[test]
    fn manifest_rejects_nonidentical_github_release_urls() {
        let mut invalid_manifest = manifest();
        invalid_manifest.download_url = "https://github.com/BanjoRecomp/BanjoRecomp-extra/releases/download/v1.0.2/BanjoRecompiled-v1.0.2-macOS.zip".into();
        assert!(invalid_manifest.validate().is_err());

        let mut invalid_manifest = manifest();
        invalid_manifest.release_url =
            "https://github.com/BanjoRecomp/BanjoRecomp/releases/tag/v1.0.2?next=evil".into();
        assert!(invalid_manifest.validate().is_err());
    }

    #[test]
    fn recipe_rejects_cross_platform_unsafe_path_components() {
        for path in [
            "CON.txt",
            "game/PRN/data",
            "game/aux ",
            "game/COM1/file",
            "game:stream",
            "game\\child",
            "game//child",
            "game*name",
            "game?name",
            "game\"name",
            "game<name",
            "game>name",
            "game|name",
            "COM¹.txt",
            "COM²",
            "LPT³.data",
        ] {
            let mut recipe = recipe();
            recipe.launch.executable = path.into();
            assert!(recipe.validate().is_err(), "{path}");
        }
    }
}
