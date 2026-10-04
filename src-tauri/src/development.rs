use crate::credentials;
use crate::models::{AdditionalBundle, PlatformPackage, PluginDiagnostics, PluginRelease};
use crate::operation_progress::OperationProgressReporter;
use anyhow::{anyhow, bail, Context, Result};
use reqwest::header::AUTHORIZATION;
use reqwest::{Client, Request, Url};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const DEV_API_ORIGIN: &str = "https://moazelgabry.com";
pub const DEV_API_BASE: &str = "https://moazelgabry.com/wp-json/moaz-releases/v1";

const HYOGEN_BUNDLE_IDENTIFIER: &str = "com.moazelgabry.hyogen.dev";
const HYOGEN_BUNDLE_NAME: &str = "Hyogen.ofx.bundle";
const HYOGEN_MODULES_BUNDLE_IDENTIFIER: &str = "com.moazelgabry.hyogen.modules";
const HYOGEN_MODULES_BUNDLE_NAME: &str = "HyogenModules.ofx.bundle";
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
    pub plugin_slug: String,
    pub plugin_name: String,
    pub access_mode: Option<String>,
    pub license_url: Option<String>,
    pub channel: String,
    pub state: String,
    pub keep_for_rollback: bool,
    pub release: PluginRelease,
}

#[derive(Debug, Clone, Default)]
pub struct ValidatedDevelopmentCatalog {
    pub email: Option<String>,
    pub plugin_grants: Option<Vec<String>>,
    pub releases: Vec<ValidatedDevelopmentRelease>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DevelopmentCatalogResponse {
    schema_version: u32,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    plugin_grants: Option<Vec<String>>,
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
    #[serde(default)]
    access_mode: Option<String>,
    #[serde(default)]
    license_url: Option<String>,
    #[serde(default)]
    bundle_metadata: Vec<DevelopmentBundleMetadata>,
}

#[derive(Debug, Deserialize, Clone)]
struct DevelopmentBundleMetadata {
    bundle_name: String,
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
        crate::access::forget_development_receipts()?;
        return Ok(None);
    };
    let client = production_client()?;
    match fetch_catalog(&client, &token).await {
        Ok(catalog) => Ok(Some(catalog)),
        Err(error) if invitation_was_rejected(&error) => {
            crate::access::forget_development_receipts()?;
            credentials::forget_invitation_token()?;
            Err(anyhow!("The development invitation was revoked or expired. Its local development receipts were cleared; connect a valid invitation to restore access."))
        }
        Err(error) => Err(error),
    }
}

pub fn invitation_was_rejected(error: &anyhow::Error) -> bool {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<reqwest::Error>())
        .and_then(reqwest::Error::status)
        .is_some_and(|status| status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN)
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

    let mut validated = ValidatedDevelopmentCatalog {
        email: catalog.email,
        plugin_grants: catalog.plugin_grants,
        ..Default::default()
    };
    for raw in catalog.releases {
        if raw.plugin.slug != "hyogen" {
            continue;
        }
        if let Some(grants) = validated.plugin_grants.as_ref() {
            if !grants.iter().any(|grant| grant == &raw.plugin.slug) {
                continue;
            }
        }
        match validate_release(raw) {
            Ok(release) => validated.releases.push(release),
            Err(warning) => validated.warnings.push(warning.to_string()),
        }
    }
    if let Some(grants) = validated.plugin_grants.as_ref() {
        if !grants.iter().any(|grant| grant == "hyogen") {
            validated
                .warnings
                .push("This invitation does not include Hyogen access.".to_string());
        } else if !validated
            .releases
            .iter()
            .any(|release| release.plugin_slug == "hyogen")
        {
            validated.warnings.push(
                "The invitation grants Hyogen, but WordPress returned no usable published Hyogen release. Check the release state and its Windows/macOS publication targets.".to_string(),
            );
        }
    }
    Ok(validated)
}

fn validate_release(raw: DevelopmentRelease) -> Result<ValidatedDevelopmentRelease> {
    if !valid_product_slug(&raw.plugin.slug) {
        bail!(
            "Ignored an invalid development grant for plugin '{}'.",
            raw.plugin.slug
        );
    }
    if raw.state != "published" && !(raw.state == "retired" && raw.keep_for_rollback) {
        bail!("Ignored a development release that is not available for installation.");
    }
    let version = semver::Version::parse(raw.version.trim_start_matches('v'))
        .context("Ignored a development release with an invalid semantic version")?;
    if !version_matches_channel(&version, &raw.channel) {
        bail!("Ignored a development release whose version did not match its channel.");
    }
    if raw.plugin.name.trim().is_empty() {
        bail!("Ignored a development grant with a missing display name.");
    }

    let bundle_metadata = development_bundle_metadata(&raw.plugin);
    validate_bundle_metadata(&bundle_metadata)?;
    if bundle_metadata
        .first()
        .map(|bundle| bundle.bundle_identifier.as_str())
        != Some(raw.plugin.bundle_identifier.as_str())
    {
        bail!("Ignored a development release whose primary bundle metadata did not match the plugin.");
    }

    let mut targets = HashSet::new();
    let mut packages = Vec::new();
    for artifact in raw.artifacts {
        let target = (artifact.platform.as_str(), artifact.architecture.as_str());
        let (package_type, install_path, required_suffix) = match target {
            ("windows", "x86_64") => ("zip", r"C:\Program Files\Common Files\OFX\Plugins", ".zip"),
            ("macos", "universal") => ("zip", "/Library/OFX/Plugins", ".zip"),
            ("linux", "x86_64") => ("tar.gz", "/usr/OFX/Plugins", ".tar.gz"),
            _ => bail!("Ignored a development release containing an unsupported artifact target."),
        };
        if !artifact
            .filename
            .to_ascii_lowercase()
            .ends_with(required_suffix)
        {
            bail!("Ignored a development release containing an unexpected package type.");
        }
        if artifact.sha256.len() != 64
            || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("Ignored a development release containing an invalid artifact checksum.");
        }
        if !targets.insert(format!("{}/{}", artifact.platform, artifact.architecture)) {
            bail!("Ignored a development release containing duplicate artifact targets.");
        }
        let primary = bundle_metadata
            .first()
            .ok_or_else(|| anyhow!("Ignored a development release with no bundle metadata."))?;
        packages.push(PlatformPackage {
            platform: artifact.platform,
            arch: artifact.architecture,
            download_url: String::new(),
            sha256: artifact.sha256.to_ascii_lowercase(),
            package_type: package_type.to_string(),
            bundle_name: primary.bundle_name.clone(),
            bundle_identifier: primary.bundle_identifier.clone(),
            additional_bundles: bundle_metadata.iter().skip(1).cloned().collect(),
            install_path: install_path.to_string(),
            min_manager_version: "0.1.25".to_string(),
            host_processes: HOST_PROCESSES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            protected_artifact_id: Some(artifact.id),
        });
    }

    if packages.is_empty() {
        bail!("Ignored a development release with no supported platform artifacts.");
    }

    Ok(ValidatedDevelopmentRelease {
        plugin_slug: raw.plugin.slug,
        plugin_name: raw.plugin.name,
        access_mode: raw.plugin.access_mode,
        license_url: raw.plugin.license_url,
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

fn valid_product_slug(slug: &str) -> bool {
    let slug = slug.trim();
    !slug.is_empty()
        && slug.len() <= 100
        && slug
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-'))
}

fn development_bundle_metadata(plugin: &DevelopmentPlugin) -> Vec<AdditionalBundle> {
    if !plugin.bundle_metadata.is_empty() {
        return plugin
            .bundle_metadata
            .iter()
            .map(|bundle| AdditionalBundle {
                bundle_name: bundle.bundle_name.clone(),
                bundle_identifier: bundle.bundle_identifier.clone(),
            })
            .collect();
    }
    if plugin.slug == "hyogen" && plugin.bundle_identifier == HYOGEN_BUNDLE_IDENTIFIER {
        return vec![
            AdditionalBundle {
                bundle_name: HYOGEN_BUNDLE_NAME.to_string(),
                bundle_identifier: HYOGEN_BUNDLE_IDENTIFIER.to_string(),
            },
            AdditionalBundle {
                bundle_name: HYOGEN_MODULES_BUNDLE_NAME.to_string(),
                bundle_identifier: HYOGEN_MODULES_BUNDLE_IDENTIFIER.to_string(),
            },
        ];
    }
    vec![AdditionalBundle {
        bundle_name: format!("{}.ofx.bundle", plugin.name.trim()),
        bundle_identifier: plugin.bundle_identifier.clone(),
    }]
}

fn validate_bundle_metadata(metadata: &[AdditionalBundle]) -> Result<()> {
    if metadata.is_empty() {
        bail!("Ignored a development release with no bundle metadata.");
    }
    let mut names = HashSet::new();
    let mut identifiers = HashSet::new();
    for bundle in metadata {
        if bundle.bundle_name.trim().is_empty()
            || !bundle.bundle_name.ends_with(".ofx.bundle")
            || bundle.bundle_name.contains('/')
            || bundle.bundle_name.contains('\\')
            || bundle.bundle_identifier.trim().is_empty()
            || !names.insert(bundle.bundle_name.clone())
            || !identifiers.insert(bundle.bundle_identifier.clone())
        {
            bail!("Ignored a development release with invalid bundle metadata.");
        }
    }
    Ok(())
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

pub async fn download_protected_artifact(
    artifact_id: u64,
    progress: &OperationProgressReporter,
) -> Result<Vec<u8>> {
    let token = credentials::invitation_token()?.ok_or_else(|| {
        anyhow!("Connect a development invitation before downloading this build.")
    })?;
    let client = production_client()?;
    progress.update(Some(10), "Requesting protected package", None);
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
    let response = client
        .execute(request)
        .await
        .context("Failed to download the protected development artifact")?
        .error_for_status()
        .context("The protected development artifact could not be downloaded")?;
    progress
        .download_response(response, 12, 70, "Downloading plugin package")
        .await
        .context("Failed to read the protected development artifact")
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
    let mut result: HashMap<String, Vec<ValidatedDevelopmentRelease>> = HashMap::new();
    for release in catalog.releases {
        result
            .entry(release.plugin_slug.clone())
            .or_default()
            .push(release);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_catalog_json() -> String {
        format!(
            r#"{{
              "schema_version": 1,
              "plugin_grants": ["hyogen"],
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
        assert!(catalog.releases[0]
            .release
            .platforms
            .iter()
            .all(|package| package.additional_bundles.len() == 1
                && package.additional_bundles[0].bundle_name == HYOGEN_MODULES_BUNDLE_NAME
                && package.additional_bundles[0].bundle_identifier
                    == HYOGEN_MODULES_BUNDLE_IDENTIFIER));
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
