use crate::installer;
use crate::models::{
    CatalogEntry, DashboardState, ManagerSummary, PlatformPackage, PluginCatalogIndex,
    PluginManifest, PluginRelease, PluginStatus, ResolvedPlugin, VersionOption,
};
use crate::settings;
use anyhow::{anyhow, Context, Result};
use reqwest::header::{CACHE_CONTROL, PRAGMA};
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_CATALOG_URL: &str =
    "https://moazelgabry.github.io/Moaz-Elgabry-plugins/plugins/index.json";

#[derive(Debug, Clone)]
pub struct CatalogBundle {
    pub source: String,
    pub source_label: String,
    pub entries: Vec<CatalogEntry>,
    pub manifests: HashMap<String, PluginManifest>,
    pub beta_plugins: HashSet<String>,
    pub release_channels: HashMap<String, String>,
    pub development_warning: Option<String>,
    pub development_invitation_has_access: Option<bool>,
}

pub async fn build_dashboard_state() -> Result<DashboardState> {
    let app_settings = settings::load_settings()?;
    let bundle = load_catalog_bundle(&app_settings).await?;
    let install_state = installer::load_install_state()?;
    let manager = ManagerSummary {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        platform: installer::current_platform().to_string(),
        arch: installer::current_arch().to_string(),
        updater_configured: installer::updater_configured(),
        catalog_url: bundle.source_label.clone(),
        beta_releases_enabled: app_settings.beta_releases_enabled,
        development_builds_enabled: app_settings.development_builds_enabled,
        development_invitation_connected: crate::credentials::invitation_token()
            .unwrap_or(None)
            .is_some(),
        development_invitation_has_access: bundle.development_invitation_has_access,
    };

    let plugins = bundle
        .entries
        .iter()
        .filter_map(|entry| {
            let manifest = bundle.manifests.get(&entry.plugin_id)?;
            let package = select_package(&manifest.platforms).ok()?;
            Some(build_plugin_status(
                entry,
                manifest,
                package,
                &install_state,
                bundle
                    .release_channels
                    .get(&entry.plugin_id)
                    .map(String::as_str)
                    .unwrap_or("stable"),
            ))
        })
        .collect::<Vec<_>>();

    Ok(DashboardState {
        manager,
        catalog_source: bundle.source,
        development_warning: bundle.development_warning,
        plugins,
    })
}

pub async fn resolve_plugin(
    plugin_id: &str,
    requested_version: Option<&str>,
) -> Result<ResolvedPlugin> {
    let app_settings = settings::load_settings()?;
    let bundle = load_catalog_bundle(&app_settings).await?;
    let manifest = bundle
        .manifests
        .get(plugin_id)
        .cloned()
        .ok_or_else(|| anyhow!("Plugin manifest not found for `{plugin_id}`"))?;
    let release = resolve_release(&manifest, requested_version)?;
    let package = select_package(&release.platforms)?;
    Ok(ResolvedPlugin {
        manifest,
        version: release.version,
        release_notes_url: release.release_notes_url,
        package,
    })
}

fn build_plugin_status(
    entry: &CatalogEntry,
    manifest: &PluginManifest,
    package: PlatformPackage,
    install_state: &crate::models::ManagedInstallState,
    release_channel: &str,
) -> PluginStatus {
    let target_bundle = PathBuf::from(&package.install_path).join(&package.bundle_name);
    let mut managed_bundles = vec![(package.bundle_name.as_str(), target_bundle.as_path())];
    let additional_paths = package
        .additional_bundles
        .iter()
        .map(|bundle| {
            (
                bundle.bundle_name.as_str(),
                PathBuf::from(&package.install_path).join(&bundle.bundle_name),
            )
        })
        .collect::<Vec<_>>();
    managed_bundles.extend(
        additional_paths
            .iter()
            .map(|(name, path)| (*name, path.as_path())),
    );
    let installed = managed_bundles.iter().all(|(_, path)| path.is_dir());
    let records = managed_bundles
        .iter()
        .map(|(_, path)| {
            let key = installer::install_key(&entry.plugin_id, path);
            install_state.installs.get(&key)
        })
        .collect::<Vec<_>>();
    let stamps = managed_bundles
        .iter()
        .map(|(_, path)| installer::read_bundle_install_stamp(path).ok().flatten())
        .collect::<Vec<_>>();
    let record = records.first().copied().flatten();
    let stamp = stamps.first().and_then(Option::as_ref);
    let installed_version = stamp
        .as_ref()
        .map(|item| item.installed_version.clone())
        .or_else(|| record.map(|item| item.installed_version.clone()));
    let managed_install = records
        .iter()
        .zip(stamps.iter())
        .all(|(record, stamp)| record.is_some() || stamp.is_some());
    let installed_is_prerelease = installed_version
        .as_ref()
        .map(|current| is_prerelease_like(current))
        .unwrap_or(false);
    let installed_newer_than_manifest = installed
        && managed_install
        && installed_version
            .as_ref()
            .map(|current| version_cmp(current, &manifest.version) == Ordering::Greater)
            .unwrap_or(false);
    let channel_switch_mode = if installed && managed_install {
        determine_channel_switch_mode(installed_version.as_deref(), &manifest.version)
            .map(str::to_string)
    } else {
        None
    };
    let channel_switch_available = channel_switch_mode.is_some();
    let catalog_behind_installed = installed_newer_than_manifest && !installed_is_prerelease;
    let needs_update = installed
        && installed_version
            .as_ref()
            .map(|current| version_cmp(current, &manifest.version) == Ordering::Less)
            .unwrap_or(true);
    let available_versions = version_options(manifest, installed_version.as_deref());

    let status = if !installed {
        "Ready to install".to_string()
    } else if channel_switch_mode.as_deref() == Some("stable_update_available") {
        "Stable update available".to_string()
    } else if channel_switch_mode.as_deref() == Some("return_to_stable") {
        "Beta installed".to_string()
    } else if catalog_behind_installed {
        "Catalog behind".to_string()
    } else if needs_update {
        "Update available".to_string()
    } else if managed_install {
        "Up to date".to_string()
    } else {
        "Unmanaged install".to_string()
    };

    PluginStatus {
        plugin_id: entry.plugin_id.clone(),
        display_name: manifest.display_name.clone(),
        icon_url: manifest.icon_url.clone().or(entry.icon_url.clone()),
        latest_version: manifest.version.clone(),
        beta_release: release_channel == "beta",
        release_channel: release_channel.to_string(),
        installed_version,
        install_path: package.install_path.clone(),
        bundle_name: package.bundle_name.clone(),
        installed,
        managed_install,
        needs_update,
        channel_switch_available,
        access_mode: crate::models::catalog_access_mode(
            &entry.plugin_id,
            entry.access_mode.as_deref().or(manifest.access_mode.as_deref()),
        ),
        license_url: entry.license_url.clone().or_else(|| manifest.license_url.clone()),
        channel_switch_mode,
        catalog_behind_installed,
        status,
        release_notes_url: manifest.release_notes_url.clone(),
        release_highlights: manifest.release_highlights.clone(),
        diagnostics: diagnostics_for_current_platform(manifest),
        available_versions,
    }
}

fn diagnostics_for_current_platform(
    manifest: &PluginManifest,
) -> Option<crate::models::PluginDiagnostics> {
    let diagnostics = manifest.diagnostics.clone()?;
    if !diagnostics.enabled {
        return None;
    }
    if !diagnostics
        .log_source_path
        .contains_key(installer::current_platform())
    {
        return None;
    }
    Some(diagnostics)
}

fn version_cmp(left: &str, right: &str) -> Ordering {
    match (parse_loose_version(left), parse_loose_version(right)) {
        (Some(left), Some(right)) => compare_loose_versions(&left, &right),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => left.cmp(right),
    }
}

pub fn select_package(packages: &[PlatformPackage]) -> Result<PlatformPackage> {
    let platform = installer::current_platform();
    let arch = installer::current_arch();

    let mut exact = None;
    let mut universal = None;

    for package in packages {
        if package.platform != platform {
            continue;
        }
        if package.arch == arch {
            exact = Some(package.clone());
            break;
        }
        if package.arch == "universal" {
            universal = Some(package.clone());
        }
    }

    exact
        .or(universal)
        .ok_or_else(|| anyhow!("No supported package found for {} / {}", platform, arch))
}

fn version_options(
    manifest: &PluginManifest,
    installed_version: Option<&str>,
) -> Vec<VersionOption> {
    let mut releases = collect_releases(manifest);
    releases.retain(|release| select_package(&release.platforms).is_ok());
    releases.sort_by(|left, right| version_cmp(&right.version, &left.version));

    let mut options = Vec::new();
    for release in releases {
        let version = release.version.clone();
        let is_current_latest = version == manifest.version;
        let is_installed = installed_version == Some(version.as_str());
        let label = if is_current_latest && is_installed {
            format!("{} (Latest installed)", version)
        } else if is_current_latest {
            format!("{} (Latest)", version)
        } else if is_installed {
            format!("{} (Installed)", version)
        } else {
            version.clone()
        };
        let action_label = version_option_action_label(
            &version,
            &manifest.version,
            installed_version,
            is_current_latest,
        );
        let channel = channel_for_version(&version).to_string();
        options.push(VersionOption {
            version,
            label,
            release_date: release.release_date,
            release_notes_url: release.release_notes_url,
            release_highlights: release.release_highlights.clone(),
            is_current_latest,
            is_installed,
            action_label,
            channel,
        });
    }
    options
}

fn channel_for_version(version: &str) -> &'static str {
    if version.to_ascii_lowercase().contains("dev") {
        "dev"
    } else if is_prerelease_like(version) {
        "beta"
    } else {
        "stable"
    }
}

fn determine_channel_switch_mode(
    installed_version: Option<&str>,
    latest_version: &str,
) -> Option<&'static str> {
    let current = installed_version?;
    if !is_prerelease_like(current) || is_prerelease_like(latest_version) {
        return None;
    }

    match version_cmp(current, latest_version) {
        Ordering::Greater => Some("return_to_stable"),
        Ordering::Less => Some("stable_update_available"),
        Ordering::Equal => Some("return_to_stable"),
    }
}

fn version_option_action_label(
    version: &str,
    latest_version: &str,
    installed_version: Option<&str>,
    is_current_latest: bool,
) -> String {
    if channel_for_version(version) == "dev" {
        return match installed_version {
            Some(current) if current == version => "Reinstall this development build".to_string(),
            Some(current) => match version_cmp(version, current) {
                Ordering::Greater if is_current_latest => {
                    "Install latest development build".to_string()
                }
                Ordering::Greater => "Install selected development build".to_string(),
                Ordering::Less => "Roll back to development build".to_string(),
                Ordering::Equal => "Install selected development build".to_string(),
            },
            None => "Install selected development build".to_string(),
        };
    }
    let installed_newer_than_target = installed_version
        .map(|current| version_cmp(current, latest_version) == Ordering::Greater)
        .unwrap_or(false);
    let installed_is_prerelease = installed_version.map(is_prerelease_like).unwrap_or(false);
    let latest_is_prerelease = is_prerelease_like(latest_version);
    let version_is_prerelease = is_prerelease_like(version);

    match installed_version {
        Some(current) if current == version => "Reinstall this version".to_string(),
        Some(current) if installed_is_prerelease && !latest_is_prerelease && is_current_latest => {
            match version_cmp(version, current) {
                Ordering::Greater | Ordering::Equal => "Install latest stable".to_string(),
                Ordering::Less => "Return to stable".to_string(),
            }
        }
        Some(_) if installed_is_prerelease && !latest_is_prerelease => {
            "Install selected stable".to_string()
        }
        Some(current) if installed_is_prerelease && version_is_prerelease => {
            match version_cmp(version, current) {
                Ordering::Greater => {
                    if is_current_latest {
                        "Install latest beta".to_string()
                    } else {
                        "Install selected beta".to_string()
                    }
                }
                Ordering::Less => "Roll back to selected beta".to_string(),
                Ordering::Equal => "Install selected beta".to_string(),
            }
        }
        Some(_) if installed_is_prerelease => "Install selected stable".to_string(),
        Some(_) if installed_newer_than_target && is_current_latest => {
            "Return to stable".to_string()
        }
        Some(_) if installed_newer_than_target => "Install selected version".to_string(),
        Some(current) => match version_cmp(version, current) {
            Ordering::Greater => "Install selected upgrade".to_string(),
            Ordering::Less => "Roll back to selected".to_string(),
            Ordering::Equal => "Install selected".to_string(),
        },
        None => "Install selected".to_string(),
    }
}

fn is_prerelease_like(version: &str) -> bool {
    if let Some(parsed) = parse_loose_version(version) {
        return parsed.prerelease.is_some();
    }

    let lowered = version.to_ascii_lowercase();
    ["beta", "alpha", "preview", "rc", "pre"]
        .iter()
        .any(|token| lowered.contains(token))
}

#[derive(Debug, Clone)]
struct LooseVersion {
    core: Vec<u64>,
    prerelease: Option<LoosePrerelease>,
}

#[derive(Debug, Clone)]
struct LoosePrerelease {
    label_rank: u8,
    label: String,
    number: Option<u64>,
}

fn parse_loose_version(raw: &str) -> Option<LooseVersion> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut numeric_parts = Vec::new();
    let mut current = String::new();
    let mut prerelease_start = None;
    let chars: Vec<char> = trimmed.chars().collect();

    for (index, ch) in chars.iter().enumerate() {
        if ch.is_ascii_digit() {
            current.push(*ch);
            continue;
        }

        if *ch == '.' {
            if current.is_empty() {
                return None;
            }
            numeric_parts.push(current.parse::<u64>().ok()?);
            current.clear();
            continue;
        }

        prerelease_start = Some(index);
        break;
    }

    if !current.is_empty() {
        numeric_parts.push(current.parse::<u64>().ok()?);
    }

    if numeric_parts.is_empty() {
        return None;
    }

    while numeric_parts.len() < 3 {
        numeric_parts.push(0);
    }

    let prerelease =
        prerelease_start.and_then(|index| parse_prerelease_fragment(&trimmed[index..]));

    Some(LooseVersion {
        core: numeric_parts,
        prerelease,
    })
}

fn parse_prerelease_fragment(raw: &str) -> Option<LoosePrerelease> {
    let normalized = raw
        .trim()
        .trim_start_matches(['-', '_', '.', ' '])
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    let split_index = normalized
        .find(|ch: char| ch.is_ascii_digit())
        .unwrap_or(normalized.len());
    let label = normalized[..split_index]
        .trim_matches(['-', '_', '.', ' '])
        .to_string();
    let number = normalized[split_index..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();

    let label_rank = match label.as_str() {
        "alpha" => 0,
        "beta" => 1,
        "preview" => 2,
        "pre" => 3,
        "rc" => 4,
        _ => 5,
    };

    Some(LoosePrerelease {
        label_rank,
        label,
        number: if number.is_empty() {
            None
        } else {
            number.parse::<u64>().ok()
        },
    })
}

fn compare_loose_versions(left: &LooseVersion, right: &LooseVersion) -> Ordering {
    let core_len = left.core.len().max(right.core.len());
    for index in 0..core_len {
        let left_part = *left.core.get(index).unwrap_or(&0);
        let right_part = *right.core.get(index).unwrap_or(&0);
        match left_part.cmp(&right_part) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }

    match (&left.prerelease, &right.prerelease) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left_pre), Some(right_pre)) => match left_pre.label_rank.cmp(&right_pre.label_rank) {
            Ordering::Equal => match left_pre.label.cmp(&right_pre.label) {
                Ordering::Equal => left_pre
                    .number
                    .unwrap_or(0)
                    .cmp(&right_pre.number.unwrap_or(0)),
                ordering => ordering,
            },
            ordering => ordering,
        },
    }
}

fn collect_releases(manifest: &PluginManifest) -> Vec<PluginRelease> {
    let mut releases = Vec::with_capacity(1 + manifest.available_versions.len());
    releases.push(PluginRelease {
        version: manifest.version.clone(),
        release_date: manifest.release_date.clone(),
        release_notes_url: manifest.release_notes_url.clone(),
        release_highlights: manifest.release_highlights.clone(),
        diagnostics: manifest.diagnostics.clone(),
        platforms: manifest.platforms.clone(),
    });

    for release in &manifest.available_versions {
        if releases.iter().any(|item| item.version == release.version) {
            continue;
        }
        releases.push(release.clone());
    }
    releases
}

fn resolve_release(
    manifest: &PluginManifest,
    requested_version: Option<&str>,
) -> Result<PluginRelease> {
    let releases = collect_releases(manifest);
    if let Some(version) = requested_version {
        return releases
            .into_iter()
            .find(|release| release.version == version)
            .ok_or_else(|| anyhow!("Plugin version `{version}` was not found in the manifest"));
    }

    Ok(releases
        .into_iter()
        .find(|release| release.version == manifest.version)
        .ok_or_else(|| anyhow!("Latest plugin version was not found in the manifest"))?)
}

async fn load_catalog_bundle(app_settings: &settings::AppSettings) -> Result<CatalogBundle> {
    let mut bundle = load_public_catalog_bundle(app_settings.beta_releases_enabled).await?;
    if !app_settings.development_builds_enabled {
        return Ok(bundle);
    }

    match crate::development::catalog_from_credential().await {
        Ok(Some(catalog)) => {
            bundle.development_invitation_has_access = catalog
                .plugin_grants
                .as_ref()
                .map(|plugin_grants| !plugin_grants.is_empty());
            if let Some(plugin_grants) = catalog.plugin_grants.as_deref() {
                if let Err(error) = crate::access::retain_development_receipts(plugin_grants) {
                    bundle.development_warning = Some(format!(
                        "The invitation was refreshed, but local development receipts could not be synchronized: {error}"
                    ));
                }
            }
            overlay_development_catalog(&mut bundle, catalog);
        }
        Ok(None) => {
            bundle.development_warning = Some(
                "Connect a development invitation to load protected development builds."
                    .to_string(),
            );
        }
        Err(error) => {
            bundle.development_warning = Some(if crate::development::invitation_was_rejected(&error) {
                format!("{error} Public stable and beta releases remain available.")
            } else {
                format!(
                    "Development builds could not be refreshed. Public stable and beta releases remain available. {error}"
                )
            });
        }
    }
    Ok(bundle)
}

async fn load_public_catalog_bundle(prefer_beta: bool) -> Result<CatalogBundle> {
    if cfg!(debug_assertions) {
        if let Some(bundle) = load_local_dev_catalog(prefer_beta)? {
            return Ok(bundle);
        }
    }

    let client = Client::builder()
        .user_agent("MoazElgabryPlugins/0.1.0")
        .build()
        .context("Failed to create HTTP client")?;

    let index = fetch_json::<PluginCatalogIndex>(&client, DEFAULT_CATALOG_URL)
        .await
        .context("Failed to load the remote plugin catalog index")?;

    let mut entries = Vec::new();
    let mut manifests = HashMap::new();
    let mut beta_plugins = HashSet::new();
    let mut release_channels = HashMap::new();
    for entry in &index.plugins {
        if let Some((manifest, beta_release)) =
            load_manifest_for_entry(&client, entry, prefer_beta).await?
        {
            if beta_release {
                beta_plugins.insert(entry.plugin_id.clone());
            }
            release_channels.insert(
                entry.plugin_id.clone(),
                if beta_release { "beta" } else { "stable" }.to_string(),
            );
            manifests.insert(entry.plugin_id.clone(), manifest);
            entries.push(entry.clone());
        }
    }

    Ok(CatalogBundle {
        source: "remote".to_string(),
        source_label: DEFAULT_CATALOG_URL.to_string(),
        entries,
        manifests,
        beta_plugins,
        release_channels,
        development_warning: None,
        development_invitation_has_access: None,
    })
}

#[derive(Debug, Clone)]
struct ChannelRelease {
    channel: String,
    release: PluginRelease,
}

fn overlay_development_catalog(
    bundle: &mut CatalogBundle,
    catalog: crate::development::ValidatedDevelopmentCatalog,
) {
    let warnings = catalog.warnings.clone();
    let by_plugin = crate::development::releases_by_plugin(catalog);
    for (plugin_id, development_releases) in by_plugin {
        if development_releases.is_empty() {
            continue;
        }
        let development_name = development_releases
            .first()
            .map(|release| release.plugin_name.clone())
            .unwrap_or_else(|| plugin_id.clone());
        let development_access_mode = development_releases
            .first()
            .and_then(|release| release.access_mode.clone());
        let development_license_url = development_releases
            .first()
            .and_then(|release| release.license_url.clone());

        let existing = bundle.manifests.get(&plugin_id).cloned();
        let public_channel = bundle
            .release_channels
            .get(&plugin_id)
            .cloned()
            .unwrap_or_else(|| "stable".to_string());
        let mut candidates = Vec::new();
        let mut all_releases = Vec::new();

        if let Some(manifest) = &existing {
            let public_releases = collect_releases(manifest);
            if let Some(current) = public_releases
                .iter()
                .find(|release| release.version == manifest.version)
            {
                candidates.push(ChannelRelease {
                    channel: public_channel.clone(),
                    release: current.clone(),
                });
            }
            all_releases.extend(public_releases.into_iter().map(|release| ChannelRelease {
                channel: channel_for_version(&release.version).to_string(),
                release,
            }));
        }

        for development in development_releases {
            if development.state == "published" {
                candidates.push(ChannelRelease {
                    channel: development.channel.clone(),
                    release: development.release.clone(),
                });
                all_releases.push(ChannelRelease {
                    channel: development.channel,
                    release: development.release,
                });
            } else if development.state == "retired" && development.keep_for_rollback {
                all_releases.push(ChannelRelease {
                    channel: development.channel,
                    release: development.release,
                });
            }
        }

        let target = candidates
            .iter()
            .filter(|candidate| candidate.channel == "dev")
            .max_by(|left, right| version_cmp(&left.release.version, &right.release.version))
            .cloned()
            .or_else(|| candidates.into_iter().max_by(compare_channel_releases));
        let Some(target) = target else {
            continue;
        };
        let mut versions: HashMap<String, ChannelRelease> = HashMap::new();
        for candidate in all_releases {
            let should_replace = versions
                .get(&candidate.release.version)
                .map(|current| {
                    channel_priority(&candidate.channel) < channel_priority(&current.channel)
                })
                .unwrap_or(true);
            if should_replace {
                versions.insert(candidate.release.version.clone(), candidate);
            }
        }
        let mut available_versions = versions
            .into_values()
            .filter(|candidate| candidate.release.version != target.release.version)
            .map(|candidate| candidate.release)
            .collect::<Vec<_>>();
        available_versions.sort_by(|left, right| version_cmp(&right.version, &left.version));

        let manifest = PluginManifest {
            plugin_id: plugin_id.clone(),
            display_name: existing
                .as_ref()
                .map(|manifest| manifest.display_name.clone())
                .unwrap_or_else(|| development_name.clone()),
            icon_url: existing
                .as_ref()
                .and_then(|manifest| manifest.icon_url.clone()),
            version: target.release.version.clone(),
            release_date: target.release.release_date.clone(),
            release_notes_url: target.release.release_notes_url.clone(),
            release_highlights: target.release.release_highlights.clone(),
            access_mode: Some(crate::models::catalog_access_mode(
                &plugin_id,
                existing
                    .as_ref()
                    .and_then(|manifest| manifest.access_mode.as_deref())
                    .or(development_access_mode.as_deref()),
            )),
            license_url: existing
                .as_ref()
                .and_then(|manifest| manifest.license_url.clone())
                .or_else(|| development_license_url.clone()),
            diagnostics: target.release.diagnostics.clone(),
            platforms: target.release.platforms.clone(),
            available_versions,
        };

        if !bundle
            .entries
            .iter()
            .any(|entry| entry.plugin_id == plugin_id)
        {
            bundle.entries.push(CatalogEntry {
                plugin_id: plugin_id.clone(),
                display_name: development_name,
                manifest_url: String::new(),
                stable_manifest_url: None,
                beta_manifest_url: None,
                icon_url: None,
                access_mode: Some(crate::models::catalog_access_mode(
                    &plugin_id,
                    existing
                        .as_ref()
                        .and_then(|manifest| manifest.access_mode.as_deref())
                        .or(development_access_mode.as_deref()),
                )),
                license_url: existing
                    .as_ref()
                    .and_then(|manifest| manifest.license_url.clone())
                    .or_else(|| development_license_url.clone()),
            });
        }
        bundle.manifests.insert(plugin_id.clone(), manifest);
        bundle
            .release_channels
            .insert(plugin_id.clone(), target.channel.clone());
        if target.channel == "beta" {
            bundle.beta_plugins.insert(plugin_id.clone());
        } else {
            bundle.beta_plugins.remove(&plugin_id);
        }
        bundle.source = format!("{}+development", bundle.source);
    }

    if !warnings.is_empty() {
        bundle.development_warning = Some(warnings.join(" "));
    }
}

fn compare_channel_releases(left: &ChannelRelease, right: &ChannelRelease) -> Ordering {
    version_cmp(&left.release.version, &right.release.version)
        .then_with(|| channel_priority(&right.channel).cmp(&channel_priority(&left.channel)))
}

fn channel_priority(channel: &str) -> u8 {
    match channel {
        "stable" => 0,
        "beta" => 1,
        "dev" => 2,
        _ => u8::MAX,
    }
}

async fn load_manifest_for_entry(
    client: &Client,
    entry: &CatalogEntry,
    prefer_beta: bool,
) -> Result<Option<(PluginManifest, bool)>> {
    if let Some(stable_url) = entry_stable_manifest_url(entry) {
        let stable_manifest = fetch_json::<PluginManifest>(client, stable_url)
            .await
            .with_context(|| format!("Failed to load remote manifest for `{}`", entry.plugin_id))?;

        if prefer_beta {
            let beta_url = entry_beta_manifest_url(entry);
            if let Some(beta_url) = beta_url {
                if let Ok(beta_manifest) = fetch_json::<PluginManifest>(client, &beta_url).await {
                    if version_cmp(&beta_manifest.version, &stable_manifest.version)
                        == Ordering::Greater
                    {
                        return Ok(Some((
                            merge_beta_manifest(stable_manifest, beta_manifest),
                            true,
                        )));
                    }
                }
            }
        }

        return Ok(Some((stable_manifest, false)));
    }

    if prefer_beta {
        let beta_url = entry_beta_manifest_url(entry).unwrap_or_else(|| entry.manifest_url.clone());
        let beta_manifest = fetch_json::<PluginManifest>(client, &beta_url)
            .await
            .with_context(|| format!("Failed to load beta manifest for `{}`", entry.plugin_id))?;
        return Ok(Some((beta_manifest, true)));
    }

    Ok(None)
}

fn merge_beta_manifest(
    stable_manifest: PluginManifest,
    mut beta_manifest: PluginManifest,
) -> PluginManifest {
    let mut merged_versions = Vec::with_capacity(
        1 + stable_manifest.available_versions.len() + beta_manifest.available_versions.len(),
    );

    merged_versions.push(PluginRelease {
        version: stable_manifest.version.clone(),
        release_date: stable_manifest.release_date.clone(),
        release_notes_url: stable_manifest.release_notes_url.clone(),
        release_highlights: stable_manifest.release_highlights.clone(),
        diagnostics: stable_manifest.diagnostics.clone(),
        platforms: stable_manifest.platforms.clone(),
    });

    for release in stable_manifest
        .available_versions
        .into_iter()
        .chain(beta_manifest.available_versions.into_iter())
    {
        if merged_versions
            .iter()
            .any(|item| item.version == release.version)
        {
            continue;
        }
        merged_versions.push(release);
    }

    beta_manifest.available_versions = merged_versions;
    beta_manifest
}

fn load_local_dev_catalog(prefer_beta: bool) -> Result<Option<CatalogBundle>> {
    let manager_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow!("Unable to resolve manager root"))?;
    let dev_index_path = manager_root
        .join("docs")
        .join("plugins")
        .join("dev")
        .join("index.json");
    if !dev_index_path.exists() {
        return Ok(None);
    }

    let raw_index = fs::read_to_string(&dev_index_path)
        .with_context(|| format!("Failed to read {}", dev_index_path.display()))?;
    let expanded_index = expand_tokens(&raw_index)?;
    let index: PluginCatalogIndex = serde_json::from_str(&expanded_index)
        .with_context(|| format!("Failed to parse {}", dev_index_path.display()))?;
    let mut entries = Vec::new();
    let mut manifests = HashMap::new();
    let mut beta_plugins = HashSet::new();
    let mut release_channels = HashMap::new();
    for entry in &index.plugins {
        if let Some((manifest, beta_release)) =
            load_local_manifest_for_entry(entry, manager_root, prefer_beta)?
        {
            if beta_release {
                beta_plugins.insert(entry.plugin_id.clone());
            }
            release_channels.insert(
                entry.plugin_id.clone(),
                if beta_release { "beta" } else { "stable" }.to_string(),
            );
            manifests.insert(entry.plugin_id.clone(), manifest);
            entries.push(entry.clone());
        }
    }

    Ok(Some(CatalogBundle {
        source: "local-dev".to_string(),
        source_label: dev_index_path.display().to_string(),
        entries,
        manifests,
        beta_plugins,
        release_channels,
        development_warning: None,
        development_invitation_has_access: None,
    }))
}

fn load_local_manifest_for_entry(
    entry: &CatalogEntry,
    manager_root: &Path,
    prefer_beta: bool,
) -> Result<Option<(PluginManifest, bool)>> {
    let has_explicit_channels =
        entry.stable_manifest_url.is_some() || entry.beta_manifest_url.is_some();

    if has_explicit_channels {
        if let Some(stable_url) = entry.stable_manifest_url.as_deref() {
            let stable_manifest = read_local_manifest(stable_url, manager_root)?;
            if prefer_beta {
                if let Some(beta_url) = entry_beta_manifest_url(entry) {
                    if let Ok(beta_manifest) = read_local_manifest(&beta_url, manager_root) {
                        if version_cmp(&beta_manifest.version, &stable_manifest.version)
                            == Ordering::Greater
                        {
                            return Ok(Some((
                                merge_beta_manifest(stable_manifest, beta_manifest),
                                true,
                            )));
                        }
                    }
                }
            }
            return Ok(Some((stable_manifest, false)));
        }

        if prefer_beta {
            let beta_url = entry_beta_manifest_url(entry)
                .ok_or_else(|| anyhow!("{} has no local beta manifest URL", entry.plugin_id))?;
            return Ok(Some((read_local_manifest(&beta_url, manager_root)?, true)));
        }

        return Ok(None);
    }

    Ok(Some((
        read_local_manifest(&entry.manifest_url, manager_root)?,
        false,
    )))
}

fn read_local_manifest(raw_url: &str, manager_root: &Path) -> Result<PluginManifest> {
    let manifest_path = resolve_manifest_path(raw_url, manager_root)?;
    let raw_manifest = fs::read_to_string(&manifest_path)
        .with_context(|| format!("Failed to read {}", manifest_path.display()))?;
    let expanded = expand_tokens(&raw_manifest)?;
    serde_json::from_str(&expanded)
        .with_context(|| format!("Failed to parse {}", manifest_path.display()))
}

async fn fetch_json<T>(client: &Client, url: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    if let Some(local_path) = resolve_local_path(url) {
        let raw = fs::read_to_string(&local_path)
            .with_context(|| format!("Failed to read {}", local_path.display()))?;
        return serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse JSON from {}", local_path.display()));
    }

    let request_url = cache_busted_url(url);
    let response = client
        .get(&request_url)
        .header(CACHE_CONTROL, "no-cache, no-store, must-revalidate")
        .header(PRAGMA, "no-cache")
        .send()
        .await
        .with_context(|| format!("Failed to fetch {url}"))?
        .error_for_status()
        .with_context(|| format!("Unexpected response while fetching {url}"))?;

    response
        .json::<T>()
        .await
        .with_context(|| format!("Failed to parse JSON from {url}"))
}

fn cache_busted_url(url: &str) -> String {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return url.to_string();
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}mepm_refresh={now}")
}

fn expand_tokens(raw: &str) -> Result<String> {
    let manager_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow!("Unable to resolve manager root"))?;
    let git_root = manager_root
        .parent()
        .ok_or_else(|| anyhow!("Unable to resolve GitHub root"))?;
    let me_ofx_root = git_root.join("ME_OFX");
    let ofx_workshop_root = git_root.join("OFX-Workshop");
    let lensdiff_root = std::env::var("LENSDIFF_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| git_root.join("LensDiff"));

    let mut expanded = raw.to_string();
    let mappings = [
        ("${MEPM_MANAGER_ROOT}", manager_root.display().to_string()),
        ("${ME_OFX_ROOT}", me_ofx_root.display().to_string()),
        ("${LENSDIFF_ROOT}", lensdiff_root.display().to_string()),
        (
            "${OFX_WORKSHOP_ROOT}",
            ofx_workshop_root.display().to_string(),
        ),
    ];
    for (token, replacement) in mappings {
        expanded = expanded.replace(token, &replacement.replace('\\', "\\\\"));
    }
    Ok(expanded)
}

fn resolve_manifest_path(raw: &str, manager_root: &Path) -> Result<PathBuf> {
    let expanded = expand_tokens(raw)?;
    if let Some(local_path) = resolve_local_path(&expanded) {
        return Ok(local_path);
    }

    Ok(manager_root.join(normalize_path_text(&expanded)))
}

fn resolve_local_path(raw: &str) -> Option<PathBuf> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return None;
    }
    if let Some(stripped) = raw.strip_prefix("file:///") {
        return Some(PathBuf::from(normalize_file_uri_path(stripped)));
    }
    if raw.starts_with('/') || raw.starts_with('\\') {
        return Some(PathBuf::from(normalize_path_text(raw)));
    }
    if cfg!(windows) && raw.contains(':') {
        return Some(PathBuf::from(normalize_path_text(raw)));
    }
    None
}

fn normalize_file_uri_path(raw: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        raw.replace('/', "\\")
    }

    #[cfg(not(target_os = "windows"))]
    {
        raw.to_string()
    }
}

fn normalize_path_text(raw: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        raw.replace('/', "\\")
    }

    #[cfg(not(target_os = "windows"))]
    {
        raw.replace('\\', "/")
    }
}

fn beta_manifest_url(raw: &str) -> Option<String> {
    raw.strip_suffix("/stable.json")
        .map(|base| format!("{base}/beta.json"))
        .or_else(|| {
            raw.strip_suffix("\\stable.json")
                .map(|base| format!("{base}\\beta.json"))
        })
}

fn entry_stable_manifest_url(entry: &CatalogEntry) -> Option<&str> {
    entry.stable_manifest_url.as_deref().or_else(|| {
        Some(entry.manifest_url.as_str())
            .filter(|_| beta_manifest_url(&entry.manifest_url).is_some())
    })
}

fn entry_beta_manifest_url(entry: &CatalogEntry) -> Option<String> {
    entry.beta_manifest_url.clone().or_else(|| {
        if entry.manifest_url.ends_with("/beta.json") || entry.manifest_url.ends_with("\\beta.json")
        {
            Some(entry.manifest_url.clone())
        } else {
            entry_stable_manifest_url(entry).and_then(beta_manifest_url)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_package() -> PlatformPackage {
        PlatformPackage {
            platform: installer::current_platform().to_string(),
            arch: installer::current_arch().to_string(),
            download_url: "https://example.com/plugin.zip".to_string(),
            sha256: "deadbeef".to_string(),
            package_type: "zip".to_string(),
            bundle_name: "Example.ofx.bundle".to_string(),
            bundle_identifier: "com.example.Plugin".to_string(),
            additional_bundles: Vec::new(),
            install_path: "C:\\Test\\Plugins".to_string(),
            min_manager_version: "0.1.0".to_string(),
            host_processes: Vec::new(),
            protected_artifact_id: None,
        }
    }

    fn test_release(version: &str) -> PluginRelease {
        PluginRelease {
            version: version.to_string(),
            release_date: "2026-03-30T00:00:00Z".to_string(),
            release_notes_url: format!("https://example.com/releases/{version}"),
            release_highlights: None,
            diagnostics: None,
            platforms: vec![test_package()],
        }
    }

    fn test_manifest(version: &str, available_versions: &[&str]) -> PluginManifest {
        PluginManifest {
            plugin_id: "example-plugin".to_string(),
            display_name: "Example Plugin".to_string(),
            icon_url: None,
            version: version.to_string(),
            release_date: "2026-03-30T00:00:00Z".to_string(),
            release_notes_url: format!("https://example.com/releases/{version}"),
            release_highlights: None,
            access_mode: None,
            license_url: None,
            diagnostics: None,
            platforms: vec![test_package()],
            available_versions: available_versions
                .iter()
                .map(|version| test_release(version))
                .collect(),
        }
    }

    fn test_catalog_entry(manifest_url: &str) -> CatalogEntry {
        CatalogEntry {
            plugin_id: "example-plugin".to_string(),
            display_name: "Example Plugin".to_string(),
            manifest_url: manifest_url.to_string(),
            stable_manifest_url: None,
            beta_manifest_url: None,
            icon_url: None,
            access_mode: None,
            license_url: None,
        }
    }

    fn channel_release(version: &str, channel: &str) -> ChannelRelease {
        ChannelRelease {
            channel: channel.to_string(),
            release: test_release(version),
        }
    }

    #[test]
    fn target_selection_uses_numeric_version_then_stable_beta_dev_priority() {
        let candidates = [
            channel_release("1.9.9", "stable"),
            channel_release("2.0.0-dev.1", "dev"),
        ];
        assert_eq!(
            candidates
                .iter()
                .max_by(|left, right| compare_channel_releases(left, right))
                .unwrap()
                .channel,
            "dev"
        );

        let tied = [
            channel_release("2.0.0", "stable"),
            channel_release("2.0.0-beta.2", "beta"),
            channel_release("2.0.0-dev.9", "dev"),
        ];
        assert_eq!(
            tied.iter()
                .max_by(|left, right| compare_channel_releases(left, right))
                .unwrap()
                .channel,
            "stable"
        );
    }

    #[test]
    fn development_warning_preserves_public_catalog_and_pages_url() {
        let manifest = test_manifest("1.0.0", &[]);
        let entry = test_catalog_entry(
            "https://moazelgabry.github.io/Moaz-Elgabry-plugins/plugins/example/stable.json",
        );
        let mut bundle = CatalogBundle {
            source: "remote".to_string(),
            source_label: DEFAULT_CATALOG_URL.to_string(),
            entries: vec![entry.clone()],
            manifests: HashMap::from([(entry.plugin_id.clone(), manifest)]),
            beta_plugins: HashSet::new(),
            release_channels: HashMap::from([(entry.plugin_id.clone(), "stable".to_string())]),
            development_warning: None,
            development_invitation_has_access: None,
        };
        overlay_development_catalog(
            &mut bundle,
            crate::development::ValidatedDevelopmentCatalog {
                email: None,
                plugin_grants: None,
                releases: Vec::new(),
                warnings: vec!["Ignored an unrecognized development grant.".to_string()],
            },
        );

        assert!(bundle.manifests.contains_key(&entry.plugin_id));
        assert_eq!(bundle.source_label, DEFAULT_CATALOG_URL);
        assert!(bundle.development_warning.is_some());
    }

    #[test]
    fn retired_development_release_is_selectable_but_never_the_target() {
        let mut published = crate::development::ValidatedDevelopmentRelease {
            plugin_slug: "hyogen".to_string(),
            plugin_name: "Hyogen".to_string(),
            access_mode: None,
            license_url: None,
            channel: "dev".to_string(),
            state: "published".to_string(),
            keep_for_rollback: true,
            release: test_release("2.0.0-dev.1"),
        };
        for package in &mut published.release.platforms {
            package.bundle_name = "Hyogen.ofx.bundle".to_string();
            package.bundle_identifier = "com.moazelgabry.hyogen.dev".to_string();
            package.protected_artifact_id = Some(1);
        }
        let mut retired = published.clone();
        retired.release.version = "1.8.0-dev.2".to_string();
        retired.state = "retired".to_string();

        let mut bundle = CatalogBundle {
            source: "remote".to_string(),
            source_label: DEFAULT_CATALOG_URL.to_string(),
            entries: Vec::new(),
            manifests: HashMap::new(),
            beta_plugins: HashSet::new(),
            release_channels: HashMap::new(),
            development_warning: None,
            development_invitation_has_access: None,
        };
        overlay_development_catalog(
            &mut bundle,
            crate::development::ValidatedDevelopmentCatalog {
                email: None,
                plugin_grants: None,
                releases: vec![published, retired],
                warnings: Vec::new(),
            },
        );

        let manifest = bundle.manifests.get("hyogen").unwrap();
        assert_eq!(manifest.version, "2.0.0-dev.1");
        assert!(manifest
            .available_versions
            .iter()
            .any(|release| release.version == "1.8.0-dev.2"));
    }

    #[test]
    fn beta_target_does_not_trigger_stable_channel_switch() {
        assert_eq!(
            determine_channel_switch_mode(Some("1.0.5Beta"), "1.0.6Beta"),
            None
        );
    }

    #[test]
    fn stable_target_keeps_beta_return_to_stable_guidance() {
        assert_eq!(
            determine_channel_switch_mode(Some("1.0.7Beta"), "1.0.6"),
            Some("return_to_stable")
        );
        assert_eq!(
            determine_channel_switch_mode(Some("1.0.5Beta"), "1.0.6"),
            Some("stable_update_available")
        );
    }

    #[test]
    fn beta_manifest_labels_latest_beta_as_beta() {
        let manifest = test_manifest("1.0.6Beta", &["1.0.2", "1.0.5Beta"]);
        let options = version_options(&manifest, Some("1.0.5Beta"));
        let latest = options
            .iter()
            .find(|option| option.version == "1.0.6Beta")
            .unwrap();
        let stable = options
            .iter()
            .find(|option| option.version == "1.0.2")
            .unwrap();

        assert_eq!(latest.action_label, "Install latest beta");
        assert_eq!(stable.action_label, "Install selected stable");
    }

    #[test]
    fn stable_manifest_labels_latest_release_as_stable_for_beta_installs() {
        let manifest = test_manifest("1.0.6", &["1.0.2"]);
        let options = version_options(&manifest, Some("1.0.5Beta"));
        let latest = options
            .iter()
            .find(|option| option.version == "1.0.6")
            .unwrap();

        assert_eq!(latest.action_label, "Install latest stable");
    }

    #[test]
    fn beta_only_index_entry_has_no_inferred_stable_manifest() {
        let entry = test_catalog_entry(
            "https://moazelgabry.github.io/Moaz-Elgabry-plugins/plugins/lensdiff/beta.json",
        );

        assert_eq!(entry_stable_manifest_url(&entry), None);
        assert_eq!(
            entry_beta_manifest_url(&entry),
            Some(entry.manifest_url.clone())
        );
    }

    #[test]
    fn explicit_beta_only_entry_is_hidden_when_beta_is_disabled() {
        let mut entry = test_catalog_entry("local-lensdiff.json");
        entry.beta_manifest_url = Some("local-lensdiff.json".to_string());

        assert_eq!(entry_stable_manifest_url(&entry), None);
        assert_eq!(
            entry_beta_manifest_url(&entry),
            Some("local-lensdiff.json".to_string())
        );
    }

    #[test]
    fn legacy_stable_index_entry_can_infer_beta_manifest() {
        let entry = test_catalog_entry(
            "https://moazelgabry.github.io/Moaz-Elgabry-plugins/plugins/lensdiff/stable.json",
        );

        assert_eq!(
            entry_stable_manifest_url(&entry),
            Some(entry.manifest_url.as_str())
        );
        assert_eq!(
            entry_beta_manifest_url(&entry),
            Some(
                "https://moazelgabry.github.io/Moaz-Elgabry-plugins/plugins/lensdiff/beta.json"
                    .to_string()
            )
        );
    }
}
