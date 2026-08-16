use crate::credentials;
use crate::models::{PlatformPackage, PluginDiagnostics, PluginRelease};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::header::AUTHORIZATION;
use reqwest::{Client, Request, Url};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const DEV_API_ORIGIN: &str = "https://moazelgabry.com";
pub const DEV_API_BASE: &str = "https://moazelgabry.com/wp-json/moaz-releases/v1";

const HYOGEN_SLUG: &str = "hyogen";
const HYOGEN_BUNDLE_IDENTIFIER: &str = "com.moazelgabry.hyogen.dev";
const HYOGEN_BUNDLE_NAME: &str = "Hyogen.ofx.bundle";
const HOST_PROCESSES: &[&str] = &[
    "Resolve",
    "DaVinci Resolve",
    "Fusion",
    "Fusion Studio",
    "Nuke",
    "NukeX",
    "NukeStudio",
    "Hiero",
];

#[derive(Debug, Clone)]
pub struct ValidatedDevelopmentRelease {
    pub channel: String,
    pub state: String,
    pub keep_for_rollback: bool,
    pub release: PluginRelease,
}

#[derive(Debug, Clone, Default)]
pub struct ValidatedDevelopmentCatalog {
    pub releases: Vec<ValidatedDevelopmentRelease>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DevelopmentCatalogResponse {
    schema_version: u32,
    releases: Vec<DevelopmentRelease>,
}

#[derive(Debug, Deserialize)]
struct DevelopmentRelease {
    plugin: DevelopmentPlugin,
    version: String,
    channel: String,
    state: String,
    #[serde(default)]
    highlights: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    diagnostics: Option<PluginDiagnostics>,
    #[serde(default)]
    keep_for_rollback: bool,
    published_at: Option<String>,
    artifacts: Vec<DevelopmentArtifact>,
}

#[derive(Debug, Deserialize)]
struct DevelopmentPlugin {
    slug: String,
    name: String,
    bundle_identifier: String,
}

#[derive(Debug, Deserialize)]
struct DevelopmentArtifact {
    id: u64,
    platform: String,
    architecture: String,
    filename: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct DownloadTicketResponse {
    ticket: String,
    download_path: String,
}

#[derive(Debug, Serialize)]
struct DownloadTicketRequest {
    artifact_id: u64,
}

pub async fn validate_token_and_catalog(token: &str) -> Result<ValidatedDevelopmentCatalog> {
    let token = credentials::validate_invitation_token(token)?;
    let client = production_client()?;
    fetch_catalog(&client, token).await
}

pub async fn catalog_from_credential() -> Result<Option<ValidatedDevelopmentCatalog>> {
    let Some(token) = credentials::invitation_token()? else {
        return Ok(None);
    };
    let client = production_client()?;
    fetch_catalog(&client, &token).await.map(Some)
}

async fn fetch_catalog(client: &Client, token: &str) -> Result<ValidatedDevelopmentCatalog> {
    let response = client
        .get(format!("{DEV_API_BASE}/dev/catalog"))
        .bearer_auth(token)
        .send()
        .await
        .context("Failed to contact the development catalog")?
        .error_for_status()
        .context("The development invitation was rejected or the catalog is unavailable")?;
    let catalog = response
        .json::<DevelopmentCatalogResponse>()
        .await
        .context("The development catalog response was invalid")?;
    validate_catalog(catalog)
}

fn validate_catalog(catalog: DevelopmentCatalogResponse) -> Result<ValidatedDevelopmentCatalog> {
    if catalog.schema_version != 1 {
        bail!(
            "Unsupported development catalog schema version {}.",
            catalog.schema_version
        );
    }

    let mut validated = ValidatedDevelopmentCatalog::default();
    for raw in catalog.releases {
        match validate_release(raw) {
            Ok(release) => validated.releases.push(release),
            Err(warning) => validated.warnings.push(warning.to_string()),
        }
    }
    Ok(validated)
}

fn validate_release(raw: DevelopmentRelease) -> Result<ValidatedDevelopmentRelease> {
    if raw.plugin.slug != HYOGEN_SLUG {
        bail!(
            "Ignored an unrecognized development grant for plugin '{}'.",
            raw.plugin.slug
        );
    }
    if raw.plugin.bundle_identifier != HYOGEN_BUNDLE_IDENTIFIER {
        bail!("Ignored a Hyogen grant whose bundle identity did not match this manager.");
    }
    if raw.state != "published" && !(raw.state == "retired" && raw.keep_for_rollback) {
        bail!("Ignored a Hyogen release that is not available for installation.");
    }
    let version = semver::Version::parse(raw.version.trim_start_matches('v'))
        .context("Ignored a Hyogen release with an invalid semantic version")?;
    if !version_matches_channel(&version, &raw.channel) {
        bail!("Ignored a Hyogen release whose version did not match its channel.");
    }
    if raw.plugin.name.trim().is_empty() {
        bail!("Ignored a Hyogen grant with a missing display name.");
    }

    let mut targets = HashSet::new();
    let mut packages = Vec::new();
    for artifact in raw.artifacts {
        let target = (artifact.platform.as_str(), artifact.architecture.as_str());
        let (package_type, install_path, required_suffix) = match target {
            ("windows", "x86_64") => ("zip", r"C:\Program Files\Common Files\OFX\Plugins", ".zip"),
            ("macos", "universal") => ("zip", "/Library/OFX/Plugins", ".zip"),
            ("linux", "x86_64") => ("tar.gz", "/usr/OFX/Plugins", ".tar.gz"),
            _ => bail!("Ignored a Hyogen release containing an unsupported artifact target."),
        };
        if !artifact
            .filename
            .to_ascii_lowercase()
            .ends_with(required_suffix)
        {
            bail!("Ignored a Hyogen release containing an unexpected package type.");
        }
        if artifact.sha256.len() != 64
            || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("Ignored a Hyogen release containing an invalid artifact checksum.");
        }
        if !targets.insert(format!("{}/{}", artifact.platform, artifact.architecture)) {
            bail!("Ignored a Hyogen release containing duplicate artifact targets.");
        }
        packages.push(PlatformPackage {
            platform: artifact.platform,
            arch: artifact.architecture,
            download_url: String::new(),
            sha256: artifact.sha256.to_ascii_lowercase(),
            package_type: package_type.to_string(),
            bundle_name: HYOGEN_BUNDLE_NAME.to_string(),
            bundle_identifier: HYOGEN_BUNDLE_IDENTIFIER.to_string(),
            install_path: install_path.to_string(),
            min_manager_version: "0.1.25".to_string(),
            host_processes: HOST_PROCESSES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            protected_artifact_id: Some(artifact.id),
        });
    }

    let required = ["windows/x86_64", "macos/universal", "linux/x86_64"];
    if required.iter().any(|target| !targets.contains(*target)) {
        bail!("Ignored a Hyogen release that did not provide every signed package target.");
    }

    Ok(ValidatedDevelopmentRelease {
        channel: raw.channel,
        state: raw.state,
        keep_for_rollback: raw.keep_for_rollback,
        release: PluginRelease {
            version: raw.version,
            release_date: raw.published_at.unwrap_or_default(),
            release_notes_url: String::new(),
            release_highlights: first_nonempty(raw.highlights, raw.notes),
            diagnostics: raw.diagnostics,
            platforms: packages,
        },
    })
}

fn version_matches_channel(version: &semver::Version, channel: &str) -> bool {
    match channel {
        "stable" => version.pre.is_empty(),
        "beta" => numbered_prerelease(version.pre.as_str(), "beta."),
        "dev" => version.build.is_empty() && numbered_prerelease(version.pre.as_str(), "dev."),
        _ => false,
    }
}

fn numbered_prerelease(value: &str, prefix: &str) -> bool {
    let Some(number) = value.strip_prefix(prefix) else {
        return false;
    };
    !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && (number == "0" || !number.starts_with('0'))
}

fn first_nonempty(highlights: String, notes: String) -> Option<String> {
    if !highlights.trim().is_empty() {
        Some(highlights)
    } else if !notes.trim().is_empty() {
        Some(notes)
    } else {
        None
    }
}

pub async fn download_protected_artifact(artifact_id: u64) -> Result<Vec<u8>> {
    let token = credentials::invitation_token()?.ok_or_else(|| {
        anyhow!("Connect a development invitation before downloading this build.")
    })?;
    let client = production_client()?;
    let ticket = client
        .post(format!("{DEV_API_BASE}/download-tickets"))
        .bearer_auth(&token)
        .json(&DownloadTicketRequest { artifact_id })
        .send()
        .await
        .context("Failed to request a protected download ticket")?
        .error_for_status()
        .context("Development artifact access is unavailable or was revoked")?
        .json::<DownloadTicketResponse>()
        .await
        .context("The protected download ticket response was invalid")?;

    let download_url = ticket_download_url(&ticket.download_path, artifact_id, &ticket.ticket)?;
    let request = build_ticket_download_request(&client, download_url)?;
    let bytes = client
        .execute(request)
        .await
        .context("Failed to download the protected development artifact")?
        .error_for_status()
        .context("The protected development artifact could not be downloaded")?
        .bytes()
        .await
        .context("Failed to read the protected development artifact")?;
    Ok(bytes.to_vec())
}

fn production_client() -> Result<Client> {
    if !DEV_API_ORIGIN.starts_with("https://") {
        bail!("The production development service must use HTTPS.");
    }
    Client::builder()
        .user_agent(concat!("MoazElgabryPlugins/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("Failed to create the development service client")
}

fn ticket_download_url(download_path: &str, artifact_id: u64, ticket: &str) -> Result<Url> {
    const DOWNLOAD_PREFIX: &str = "/wp-json/moaz-releases/v1/downloads/";
    let artifact_segment = download_path.strip_prefix(DOWNLOAD_PREFIX).ok_or_else(|| {
        anyhow!("The protected download path was not a same-origin relative path.")
    })?;
    if artifact_segment.is_empty()
        || !artifact_segment.bytes().all(|byte| byte.is_ascii_digit())
        || artifact_segment.parse::<u64>().ok() != Some(artifact_id)
    {
        bail!("The protected download path was not a same-origin relative path.");
    }
    let origin = Url::parse(DEV_API_ORIGIN)?;
    let mut url = origin
        .join(download_path)
        .context("The protected download path was invalid")?;
    if url.scheme() != "https"
        || url.host_str() != origin.host_str()
        || url.port_or_known_default() != origin.port_or_known_default()
    {
        bail!("The protected download path changed origin.");
    }
    url.query_pairs_mut().append_pair("ticket", ticket);
    Ok(url)
}

fn build_ticket_download_request(client: &Client, url: Url) -> Result<Request> {
    let request = client
        .get(url)
        .build()
        .context("Failed to prepare the protected artifact request")?;
    if request.headers().contains_key(AUTHORIZATION) {
        bail!("Protected artifact downloads must not contain bearer credentials.");
    }
    Ok(request)
}

pub fn releases_by_plugin(
    catalog: ValidatedDevelopmentCatalog,
) -> HashMap<String, Vec<ValidatedDevelopmentRelease>> {
    let mut result = HashMap::new();
    result.insert(HYOGEN_SLUG.to_string(), catalog.releases);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_catalog_json() -> String {
        format!(
            r#"{{
              "schema_version": 1,
              "releases": [{{
                "plugin": {{"slug":"hyogen","name":"Hyogen","bundle_identifier":"com.moazelgabry.hyogen.dev"}},
                "version":"2.0.0-dev.7","channel":"dev","state":"published",
                "highlights":"New controls","notes":"","keep_for_rollback":true,
                "published_at":"2026-08-16 12:00:00",
                "artifacts":[
                  {{"id":1,"platform":"windows","architecture":"x86_64","filename":"Hyogen-windows.zip","sha256":"{hash}"}},
                  {{"id":2,"platform":"macos","architecture":"universal","filename":"Hyogen-macos.zip","sha256":"{hash}"}},
                  {{"id":3,"platform":"linux","architecture":"x86_64","filename":"Hyogen-linux.tar.gz","sha256":"{hash}"}}
                ]
              }}]
            }}"#,
            hash = "a".repeat(64)
        )
    }

    #[test]
    fn signed_allowlist_accepts_only_the_expected_hyogen_identity_and_targets() {
        let raw: DevelopmentCatalogResponse = serde_json::from_str(&valid_catalog_json()).unwrap();
        let catalog = validate_catalog(raw).unwrap();
        assert_eq!(catalog.releases.len(), 1);
        assert!(catalog.warnings.is_empty());
        assert_eq!(catalog.releases[0].release.platforms.len(), 3);
        assert!(catalog.releases[0]
            .release
            .platforms
            .iter()
            .all(|package| package.protected_artifact_id.is_some()));
    }

    #[test]
    fn unknown_and_mismatched_grants_are_ignored_with_non_secret_warnings() {
        let json = valid_catalog_json()
            .replace("\"hyogen\"", "\"unknown-plugin\"")
            .replace("mer_", "should-never-appear_");
        let raw: DevelopmentCatalogResponse = serde_json::from_str(&json).unwrap();
        let catalog = validate_catalog(raw).unwrap();
        assert!(catalog.releases.is_empty());
        assert_eq!(catalog.warnings.len(), 1);
        assert!(!catalog.warnings[0].contains("mer_"));
    }

    #[test]
    fn retired_release_requires_the_rollback_marker() {
        let json = valid_catalog_json()
            .replace("\"state\":\"published\"", "\"state\":\"retired\"")
            .replace("\"keep_for_rollback\":true", "\"keep_for_rollback\":false");
        let raw: DevelopmentCatalogResponse = serde_json::from_str(&json).unwrap();
        let catalog = validate_catalog(raw).unwrap();
        assert!(catalog.releases.is_empty());
    }

    #[test]
    fn private_stable_and_beta_channels_require_matching_semver() {
        let stable_json = valid_catalog_json()
            .replace("2.0.0-dev.7", "2.0.0")
            .replace("\"channel\":\"dev\"", "\"channel\":\"stable\"");
        let stable: DevelopmentCatalogResponse = serde_json::from_str(&stable_json).unwrap();
        assert_eq!(
            validate_catalog(stable).unwrap().releases[0].channel,
            "stable"
        );

        let beta_json = valid_catalog_json()
            .replace("2.0.0-dev.7", "2.0.0-beta.3")
            .replace("\"channel\":\"dev\"", "\"channel\":\"beta\"");
        let beta: DevelopmentCatalogResponse = serde_json::from_str(&beta_json).unwrap();
        assert_eq!(validate_catalog(beta).unwrap().releases[0].channel, "beta");

        let mismatch_json =
            valid_catalog_json().replace("\"channel\":\"dev\"", "\"channel\":\"stable\"");
        let mismatch: DevelopmentCatalogResponse = serde_json::from_str(&mismatch_json).unwrap();
        assert!(validate_catalog(mismatch).unwrap().releases.is_empty());
    }

    #[test]
    fn ticket_download_is_same_origin_https_and_has_no_bearer_header() {
        let client = Client::new();
        let url = ticket_download_url(
            "/wp-json/moaz-releases/v1/downloads/12",
            12,
            "short-lived-ticket",
        )
        .unwrap();
        let request = build_ticket_download_request(&client, url).unwrap();
        assert_eq!(request.url().scheme(), "https");
        assert_eq!(request.url().host_str(), Some("moazelgabry.com"));
        assert!(!request.headers().contains_key(AUTHORIZATION));
        assert!(request.url().query().unwrap().contains("ticket="));
    }

    #[test]
    fn ticket_download_rejects_absolute_or_cross_origin_paths() {
        assert!(ticket_download_url("https://evil.example/file", 12, "ticket").is_err());
        assert!(ticket_download_url("//evil.example/file", 12, "ticket").is_err());
        assert!(
            ticket_download_url("/wp-json/moaz-releases/v1/downloads/../12", 12, "ticket").is_err()
        );
        assert!(
            ticket_download_url("/wp-json/moaz-releases/v1/downloads/12/extra", 12, "ticket")
                .is_err()
        );
        assert!(
            ticket_download_url("/wp-json/moaz-releases/v1/downloads/13", 12, "ticket").is_err()
        );
    }
}
