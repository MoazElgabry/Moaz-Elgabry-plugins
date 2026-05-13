use crate::catalog;
use crate::models::{
    BundleInstallStamp, InstallRecord, ManagedInstallState, PlatformPackage, PluginDiagnostics,
    PluginOperationResult,
};
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use flate2::read::GzDecoder;
use fs_extra::dir::{copy as copy_dir, CopyOptions};
use plist::Value as PlistValue;
use reqwest::Client;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use sysinfo::System;
use tar::Archive;
use tauri::{AppHandle, Emitter};
use tempfile::tempdir;
use walkdir::WalkDir;
use zip::ZipArchive;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const LOG_EXPORT_STEPS: [&str; 3] = [
    "Launching Resolve for diagnostics",
    "Reproduce the issue, then close Resolve",
    "Collecting logs after Resolve closes",
];

pub fn updater_configured() -> bool {
    true
}

pub fn current_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unsupported"
    }
}

pub fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => other,
    }
}

pub fn install_key(plugin_id: &str, bundle_path: &Path) -> String {
    format!("{}::{}", plugin_id, bundle_path.display())
}

pub fn load_install_state() -> Result<ManagedInstallState> {
    let state_path = install_state_path()?;
    if !state_path.exists() {
        return Ok(ManagedInstallState::default());
    }
    let raw = fs::read_to_string(&state_path)
        .with_context(|| format!("Failed to read {}", state_path.display()))?;
    serde_json::from_str(&raw).context("Failed to parse install state JSON")
}

pub fn read_bundle_install_stamp(bundle_root: &Path) -> Result<Option<BundleInstallStamp>> {
    let stamp_path = bundle_stamp_path(bundle_root);
    if stamp_path.exists() {
        let raw = fs::read_to_string(&stamp_path)
            .with_context(|| format!("Failed to read {}", stamp_path.display()))?;
        let stamp: BundleInstallStamp = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse {}", stamp_path.display()))?;
        return Ok(Some(stamp));
    }

    let legacy_path = legacy_bundle_stamp_path(bundle_root);
    if !legacy_path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&legacy_path)
        .with_context(|| format!("Failed to read {}", legacy_path.display()))?;
    let stamp: BundleInstallStamp = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse {}", legacy_path.display()))?;
    Ok(Some(stamp))
}

fn save_install_state(state: &ManagedInstallState) -> Result<()> {
    let state_path = install_state_path()?;
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(state)?;
    fs::write(&state_path, raw).with_context(|| format!("Failed to write {}", state_path.display()))
}

fn install_state_path() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .ok_or_else(|| anyhow!("Unable to resolve local application data directory"))?;
    let new_path = base.join("Moaz Elgabry Plugins").join("install-state.json");
    let legacy_path = base
        .join("MoazElgabryPluginManager")
        .join("install-state.json");

    if !new_path.exists() && legacy_path.exists() {
        if let Some(parent) = new_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        fs::copy(&legacy_path, &new_path).with_context(|| {
            format!(
                "Failed to migrate install state from {} to {}",
                legacy_path.display(),
                new_path.display()
            )
        })?;
    }

    Ok(new_path)
}

pub async fn apply_plugin_action(
    plugin_id: &str,
    action: &str,
    target_version: Option<&str>,
) -> Result<PluginOperationResult> {
    if action == "uninstall" || action == "force-uninstall" {
        return uninstall_plugin(plugin_id, action).await;
    }

    let resolved = catalog::resolve_plugin(plugin_id, target_version).await?;
    ensure_supported_package(&resolved.package)?;
    ensure_hosts_closed(&resolved.package.host_processes)?;
    let source_spec = parse_package_source_spec(&resolved.package.download_url);
    let installed_at = Utc::now().to_rfc3339();
    let staging_root = tempdir().context("Failed to create staging directory")?;
    let stage_bundle_root = staging_root.path().join(&resolved.package.bundle_name);

    let bundle_root = if resolved.package.package_type == "bundle-dir" {
        let local_bundle = resolve_local_source_path(&source_spec.source)?;
        verify_bundle(&local_bundle, &resolved.package)?;
        stage_bundle_directory(&local_bundle, &stage_bundle_root)?;
        write_bundle_install_stamp(
            &stage_bundle_root,
            plugin_id,
            &resolved.version,
            &resolved.package.bundle_identifier,
            &installed_at,
        )?;
        stage_bundle_root.clone()
    } else {
        let bytes = load_package_bytes(&source_spec.source).await?;
        verify_archive_hash(&bytes, &resolved.package.sha256)?;

        let extracted_root = staging_root.path().join("extract");
        if resolved.package.package_type == "tar.gz" {
            extract_tar_gz(&bytes, &extracted_root)?;
        } else {
            extract_zip(&bytes, &extracted_root)?;
        }

        let extracted_bundle = find_bundle_root(&extracted_root)?
            .ok_or_else(|| anyhow!("Archive did not contain an .ofx.bundle"))?;
        verify_bundle(&extracted_bundle, &resolved.package)?;
        fs::rename(&extracted_bundle, &stage_bundle_root).with_context(|| {
            format!(
                "Failed to stage extracted bundle from {} to {}",
                extracted_bundle.display(),
                stage_bundle_root.display()
            )
        })?;
        write_bundle_install_stamp(
            &stage_bundle_root,
            plugin_id,
            &resolved.version,
            &resolved.package.bundle_identifier,
            &installed_at,
        )?;
        stage_bundle_root.clone()
    };

    let install_root = PathBuf::from(&resolved.package.install_path);
    fs::create_dir_all(&install_root).ok();

    if cfg!(target_os = "windows") {
        privileged_install_windows(
            &bundle_root,
            &install_root,
            &resolved.package.bundle_name,
            source_spec.simulate_fail_after_backup,
        )?;
    } else if cfg!(target_os = "macos") {
        privileged_install_macos(
            &bundle_root,
            &install_root,
            &resolved.package.bundle_name,
            source_spec.simulate_fail_after_backup,
        )?;
    } else if cfg!(target_os = "linux") {
        privileged_install_linux(
            &bundle_root,
            &install_root,
            &resolved.package.bundle_name,
            source_spec.simulate_fail_after_backup,
        )?;
    } else {
        bail!("Only macOS, Windows, and Linux are supported in v1");
    }

    let target_bundle = install_root.join(&resolved.package.bundle_name);
    let mut state = load_install_state().unwrap_or_default();
    state.installs.insert(
        install_key(plugin_id, &target_bundle),
        InstallRecord {
            plugin_id: plugin_id.to_string(),
            bundle_path: target_bundle.display().to_string(),
            installed_version: resolved.version.clone(),
            bundle_identifier: resolved.package.bundle_identifier.clone(),
            installed_at: installed_at.clone(),
        },
    );
    save_install_state(&state)?;

    Ok(PluginOperationResult {
        plugin_id: plugin_id.to_string(),
        action: action.to_string(),
        status: "success".to_string(),
        message: format!(
            "{} {} installed to {}.",
            resolved.manifest.display_name,
            resolved.version,
            install_root.display()
        ),
    })
}

async fn uninstall_plugin(plugin_id: &str, action: &str) -> Result<PluginOperationResult> {
    let resolved = catalog::resolve_plugin(plugin_id, None).await?;
    ensure_hosts_closed(&resolved.package.host_processes)?;

    let install_root = PathBuf::from(&resolved.package.install_path);
    let target_bundle = install_root.join(&resolved.package.bundle_name);
    let install_key = install_key(plugin_id, &target_bundle);
    let mut state = load_install_state().unwrap_or_default();
    let record = state.installs.get(&install_key).cloned();
    let stamp = read_bundle_install_stamp(&target_bundle).ok().flatten();
    let managed_install = record.is_some() || stamp.is_some();
    let bundle_exists = target_bundle.exists();

    if action == "uninstall" && !managed_install {
        bail!("This uninstall requires a managed install. Use force-uninstall to remove a detected unmanaged bundle.");
    }

    if !bundle_exists && record.is_none() && stamp.is_none() {
        bail!("No installed bundle was found for this plugin.");
    }

    if bundle_exists {
        if cfg!(target_os = "windows") {
            privileged_uninstall_windows(&target_bundle, &resolved.package.bundle_name)?;
        } else if cfg!(target_os = "macos") {
            privileged_uninstall_macos(&target_bundle, &resolved.package.bundle_name)?;
        } else if cfg!(target_os = "linux") {
            privileged_uninstall_linux(&target_bundle, &resolved.package.bundle_name)?;
        } else {
            bail!("Only macOS, Windows, and Linux are supported in v1");
        }
    }

    state.installs.remove(&install_key);
    save_install_state(&state)?;

    let message = if bundle_exists {
        format!(
            "{} was uninstalled from {}.",
            resolved.manifest.display_name,
            install_root.display()
        )
    } else {
        format!(
            "{} was already missing from disk. Manager tracking was cleaned up.",
            resolved.manifest.display_name
        )
    };

    Ok(PluginOperationResult {
        plugin_id: plugin_id.to_string(),
        action: action.to_string(),
        status: "success".to_string(),
        message,
    })
}

pub async fn export_plugin_logs(
    app: &AppHandle,
    plugin_id: &str,
    destination_dir: &str,
    remove_previous_logs: bool,
) -> Result<PluginOperationResult> {
    let resolved = catalog::resolve_plugin(plugin_id, None).await?;
    let diagnostics = resolved
        .manifest
        .diagnostics
        .as_ref()
        .filter(|diagnostics| diagnostics.enabled)
        .ok_or_else(|| anyhow!("Diagnostics are not enabled for this plugin release."))?;
    let raw_log_sources = diagnostics
        .log_source_path
        .get(current_platform())
        .ok_or_else(|| {
            anyhow!(
                "Diagnostics are not enabled for platform `{}`.",
                current_platform()
            )
        })?;

    ensure_resolve_closed(&resolved.package.host_processes)?;

    let destination_root = PathBuf::from(destination_dir);
    fs::create_dir_all(&destination_root)
        .with_context(|| format!("Failed to create {}", destination_root.display()))?;
    let log_sources = raw_log_sources
        .as_slice()
        .iter()
        .map(|source| expand_diagnostics_path(source))
        .collect::<Result<Vec<_>>>()?;
    if log_sources.is_empty() {
        bail!(
            "Diagnostics are not enabled for platform `{}`.",
            current_platform()
        );
    }
    if remove_previous_logs {
        clear_log_sources(&log_sources)?;
    }
    let mut command = resolve_command()?;
    let env_entries = diagnostics_environment(diagnostics, &log_sources[0], &destination_root)?;
    command.envs(env_entries.clone());
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());

    emit_log_export_progress(app, plugin_id, LOG_EXPORT_STEPS[0], 0);
    let mut child = command
        .spawn()
        .context("Failed to launch Resolve for the diagnostics session")?;
    emit_log_export_progress(app, plugin_id, LOG_EXPORT_STEPS[1], 1);
    child
        .wait()
        .context("Failed while waiting for Resolve to close")?;
    emit_log_export_progress(app, plugin_id, LOG_EXPORT_STEPS[2], 2);

    let export_root = destination_root.join(format!(
        "{}-logs-{}",
        sanitize_export_name(&resolved.manifest.display_name),
        Utc::now().format("%Y%m%d-%H%M%S")
    ));
    let copy_outcome = copy_log_sources_to_export_or_notice(
        &log_sources,
        &export_root,
        &resolved.manifest.display_name,
        &env_entries,
    )?;

    Ok(PluginOperationResult {
        plugin_id: plugin_id.to_string(),
        action: "export-logs".to_string(),
        status: "success".to_string(),
        message: log_export_message(&copy_outcome, &export_root, &env_entries),
    })
}

pub async fn check_plugin_log_export_ready(plugin_id: &str) -> Result<()> {
    let resolved = catalog::resolve_plugin(plugin_id, None).await?;
    let diagnostics = resolved
        .manifest
        .diagnostics
        .as_ref()
        .filter(|diagnostics| diagnostics.enabled)
        .ok_or_else(|| anyhow!("Diagnostics are not enabled for this plugin release."))?;
    let raw_log_sources = diagnostics
        .log_source_path
        .get(current_platform())
        .ok_or_else(|| {
            anyhow!(
                "Diagnostics are not enabled for platform `{}`.",
                current_platform()
            )
        })?;

    if raw_log_sources.as_slice().is_empty() {
        bail!(
            "Diagnostics are not enabled for platform `{}`.",
            current_platform()
        );
    }

    ensure_resolve_closed(&resolved.package.host_processes)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogExportProgress {
    plugin_id: String,
    label: String,
    steps: Vec<String>,
    step_index: usize,
}

fn emit_log_export_progress(app: &AppHandle, plugin_id: &str, label: &str, step_index: usize) {
    let _ = app.emit(
        "plugin-log-export-progress",
        LogExportProgress {
            plugin_id: plugin_id.to_string(),
            label: label.to_string(),
            steps: LOG_EXPORT_STEPS
                .iter()
                .map(|step| step.to_string())
                .collect(),
            step_index,
        },
    );
}

fn ensure_resolve_closed(host_processes: &[String]) -> Result<()> {
    let mut candidates = host_processes.to_vec();
    candidates.extend([
        "Resolve".to_string(),
        "Resolve.exe".to_string(),
        "DaVinci Resolve".to_string(),
        "DaVinci Resolve.app".to_string(),
    ]);
    match ensure_hosts_closed(&candidates) {
        Ok(()) => Ok(()),
        Err(error) => bail!("Close Resolve before starting a diagnostics session. {error}"),
    }
}

fn diagnostics_environment(
    diagnostics: &PluginDiagnostics,
    log_source: &Path,
    destination_root: &Path,
) -> Result<Vec<(String, String)>> {
    let mut entries = Vec::new();
    for (key, value) in &diagnostics.environment {
        if !is_valid_environment_key(key) {
            bail!("Diagnostics environment key `{key}` is not valid");
        }
        let expanded = value
            .replace("{logSourcePath}", &log_source.display().to_string())
            .replace("{destinationDir}", &destination_root.display().to_string());
        entries.push((key.clone(), expanded));
    }
    Ok(entries)
}

fn is_valid_environment_key(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn resolve_command() -> Result<Command> {
    if let Some(explicit) = std::env::var_os("RESOLVE_EXECUTABLE") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Ok(Command::new(path));
        }
    }

    for candidate in default_resolve_candidates() {
        if candidate.exists() {
            return Ok(Command::new(candidate));
        }
    }

    let fallback = if cfg!(target_os = "windows") {
        "Resolve.exe"
    } else {
        "resolve"
    };
    Ok(Command::new(fallback))
}

fn default_resolve_candidates() -> Vec<PathBuf> {
    if cfg!(target_os = "windows") {
        return vec![PathBuf::from(
            r"C:\Program Files\Blackmagic Design\DaVinci Resolve\Resolve.exe",
        )];
    }
    if cfg!(target_os = "macos") {
        return vec![PathBuf::from(
            "/Applications/DaVinci Resolve/DaVinci Resolve.app/Contents/MacOS/Resolve",
        )];
    }
    if cfg!(target_os = "linux") {
        return vec![
            PathBuf::from("/opt/resolve/bin/resolve"),
            PathBuf::from("/usr/bin/resolve"),
            PathBuf::from("/usr/local/bin/resolve"),
        ];
    }
    Vec::new()
}

fn expand_diagnostics_path(raw: &str) -> Result<PathBuf> {
    let expanded_home = expand_home(raw);
    let expanded_percent = expand_percent_environment(&expanded_home)?;
    let expanded_dollar = expand_dollar_environment(&expanded_percent)?;
    Ok(PathBuf::from(normalize_runtime_path(&expanded_dollar)))
}

fn expand_home(raw: &str) -> String {
    if raw == "~" {
        return dirs::home_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| raw.to_string());
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).display().to_string();
        }
    }
    raw.to_string()
}

fn expand_percent_environment(raw: &str) -> Result<String> {
    let mut output = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find('%') {
        output.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('%') else {
            output.push('%');
            output.push_str(after_start);
            return Ok(output);
        };
        let name = &after_start[..end];
        if name.is_empty() {
            output.push_str("%%");
        } else {
            output.push_str(
                &std::env::var(name)
                    .with_context(|| format!("Environment variable `{name}` is not set"))?,
            );
        }
        rest = &after_start[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

fn expand_dollar_environment(raw: &str) -> Result<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut output = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '$' {
            output.push(chars[index]);
            index += 1;
            continue;
        }

        if chars.get(index + 1) == Some(&'{') {
            let mut end = index + 2;
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }
            if end >= chars.len() {
                output.push(chars[index]);
                index += 1;
                continue;
            }
            let name = chars[index + 2..end].iter().collect::<String>();
            output.push_str(
                &std::env::var(&name)
                    .with_context(|| format!("Environment variable `{name}` is not set"))?,
            );
            index = end + 1;
            continue;
        }

        let mut end = index + 1;
        while end < chars.len() && (chars[end] == '_' || chars[end].is_ascii_alphanumeric()) {
            end += 1;
        }
        if end == index + 1 {
            output.push('$');
            index += 1;
            continue;
        }
        let name = chars[index + 1..end].iter().collect::<String>();
        output.push_str(
            &std::env::var(&name)
                .with_context(|| format!("Environment variable `{name}` is not set"))?,
        );
        index = end;
    }
    Ok(output)
}

struct LogCopyOutcome {
    copied_files: usize,
    notice_written: bool,
}

fn copy_logs_to_export_or_notice(
    source: &Path,
    destination: &Path,
    plugin_name: &str,
    env_entries: &[(String, String)],
) -> Result<LogCopyOutcome> {
    fs::create_dir_all(destination)
        .with_context(|| format!("Failed to create {}", destination.display()))?;

    if !source.exists() {
        write_missing_log_notice(source, destination, plugin_name, env_entries)?;
        return Ok(LogCopyOutcome {
            copied_files: 0,
            notice_written: true,
        });
    }

    if source.is_file() {
        let filename = source
            .file_name()
            .ok_or_else(|| anyhow!("Log source file has no filename"))?;
        fs::copy(source, destination.join(filename))
            .with_context(|| format!("Failed to copy {}", source.display()))?;
        return Ok(LogCopyOutcome {
            copied_files: 1,
            notice_written: false,
        });
    }

    let mut copied = 0;
    for entry in WalkDir::new(source)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(source).with_context(|| {
            format!(
                "Failed to resolve relative path for {}",
                entry.path().display()
            )
        })?;
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        fs::copy(entry.path(), &target)
            .with_context(|| format!("Failed to copy {}", entry.path().display()))?;
        copied += 1;
    }

    if copied == 0 {
        write_missing_log_notice(source, destination, plugin_name, env_entries)?;
        return Ok(LogCopyOutcome {
            copied_files: 0,
            notice_written: true,
        });
    }

    Ok(LogCopyOutcome {
        copied_files: copied,
        notice_written: false,
    })
}

fn copy_log_sources_to_export_or_notice(
    sources: &[PathBuf],
    destination: &Path,
    plugin_name: &str,
    env_entries: &[(String, String)],
) -> Result<LogCopyOutcome> {
    fs::create_dir_all(destination)
        .with_context(|| format!("Failed to create {}", destination.display()))?;

    let mut copied_files = 0;
    let mut missing_sources = Vec::new();
    for source in sources {
        if !source.exists() {
            missing_sources.push(source.clone());
            continue;
        }
        copied_files +=
            copy_logs_to_export_or_notice(source, destination, plugin_name, env_entries)?
                .copied_files;
    }

    if copied_files == 0 {
        write_missing_log_sources_notice(sources, destination, plugin_name, env_entries)?;
        return Ok(LogCopyOutcome {
            copied_files: 0,
            notice_written: true,
        });
    }

    if !missing_sources.is_empty() {
        write_partial_log_sources_notice(&missing_sources, destination, plugin_name, env_entries)?;
    }

    Ok(LogCopyOutcome {
        copied_files,
        notice_written: false,
    })
}

fn clear_log_sources(sources: &[PathBuf]) -> Result<()> {
    for source in sources {
        if !source.exists() {
            continue;
        }
        ensure_safe_log_clear_path(source)?;
        let metadata = fs::symlink_metadata(source)
            .with_context(|| format!("Failed to inspect {}", source.display()))?;
        if metadata.is_file() || metadata.file_type().is_symlink() {
            fs::remove_file(source)
                .with_context(|| format!("Failed to remove previous log {}", source.display()))?;
            continue;
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(source)
                .with_context(|| format!("Failed to read {}", source.display()))?
            {
                let path = entry?.path();
                let entry_metadata = fs::symlink_metadata(&path)
                    .with_context(|| format!("Failed to inspect {}", path.display()))?;
                if entry_metadata.is_dir() && !entry_metadata.file_type().is_symlink() {
                    fs::remove_dir_all(&path).with_context(|| {
                        format!("Failed to remove previous log folder {}", path.display())
                    })?;
                } else {
                    fs::remove_file(&path).with_context(|| {
                        format!("Failed to remove previous log file {}", path.display())
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn ensure_safe_log_clear_path(source: &Path) -> Result<()> {
    let canonical_source = fs::canonicalize(source)
        .with_context(|| format!("Failed to resolve {}", source.display()))?;
    if canonical_source.parent().is_none() || canonical_source.file_name().is_none() {
        bail!(
            "Refusing to remove previous logs from broad path {}",
            source.display()
        );
    }

    for forbidden in dangerous_log_clear_roots() {
        if forbidden.exists() {
            let canonical_forbidden = fs::canonicalize(&forbidden)
                .with_context(|| format!("Failed to resolve {}", forbidden.display()))?;
            if canonical_source == canonical_forbidden {
                bail!(
                    "Refusing to remove previous logs from broad path {}",
                    source.display()
                );
            }
        }
    }
    Ok(())
}

fn dangerous_log_clear_roots() -> Vec<PathBuf> {
    [
        dirs::home_dir(),
        dirs::data_dir(),
        dirs::data_local_dir(),
        dirs::cache_dir(),
        Some(std::env::temp_dir()),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn write_missing_log_notice(
    source: &Path,
    destination: &Path,
    plugin_name: &str,
    env_entries: &[(String, String)],
) -> Result<()> {
    write_log_notice(
        &[source.to_path_buf()],
        destination,
        plugin_name,
        env_entries,
        "Resolve was launched for a diagnostics session, but no plugin log file or log files were found at the expected source path after Resolve closed.",
        "diagnostics-export-note.txt",
    )
}

fn write_missing_log_sources_notice(
    sources: &[PathBuf],
    destination: &Path,
    plugin_name: &str,
    env_entries: &[(String, String)],
) -> Result<()> {
    write_log_notice(
        sources,
        destination,
        plugin_name,
        env_entries,
        "Resolve was launched for a diagnostics session, but no plugin log file or log files were found at the expected source paths after Resolve closed.",
        "diagnostics-export-note.txt",
    )
}

fn write_partial_log_sources_notice(
    sources: &[PathBuf],
    destination: &Path,
    plugin_name: &str,
    env_entries: &[(String, String)],
) -> Result<()> {
    write_log_notice(
        sources,
        destination,
        plugin_name,
        env_entries,
        "Some configured diagnostics log sources were not found after Resolve closed.",
        "diagnostics-export-missing-sources.txt",
    )
}

fn write_log_notice(
    sources: &[PathBuf],
    destination: &Path,
    plugin_name: &str,
    env_entries: &[(String, String)],
    message: &str,
    filename: &str,
) -> Result<()> {
    let env_summary = if env_entries.is_empty() {
        "No diagnostics environment variables were configured.\n".to_string()
    } else {
        env_entries
            .iter()
            .map(|(key, value)| format!("{key}={value}\n"))
            .collect::<String>()
    };
    let source_summary = sources
        .iter()
        .map(|source| format!("- {}\n", source.display()))
        .collect::<String>();
    let notice = format!(
        "Diagnostics log export note\n\
         Plugin: {plugin_name}\n\
         Export time: {}\n\
         Expected log sources:\n{}\n\
         {}\n\n\
         Diagnostics environment used:\n{}",
        Utc::now().to_rfc3339(),
        source_summary,
        message,
        env_summary
    );
    fs::write(destination.join(filename), notice).with_context(|| {
        format!(
            "Failed to write diagnostics note in {}",
            destination.display()
        )
    })
}

fn diagnostics_environment_activity_summary(env_entries: &[(String, String)]) -> String {
    if env_entries.is_empty() {
        return "Diagnostics environment used: none.".to_string();
    }

    format!(
        "Diagnostics environment used: {}.",
        env_entries
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    )
}

fn log_export_message(
    outcome: &LogCopyOutcome,
    export_root: &Path,
    env_entries: &[(String, String)],
) -> String {
    let base = if outcome.notice_written && outcome.copied_files == 0 {
        format!(
            "No plugin log file was produced. Wrote a diagnostics note to {}.",
            export_root.display()
        )
    } else {
        format!(
            "Exported {} log file{} to {}.",
            outcome.copied_files,
            if outcome.copied_files == 1 { "" } else { "s" },
            export_root.display()
        )
    };

    format!(
        "{} {}",
        base,
        diagnostics_environment_activity_summary(env_entries)
    )
}

fn sanitize_export_name(raw: &str) -> String {
    let sanitized = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "plugin".to_string()
    } else {
        sanitized
    }
}

struct PackageSourceSpec {
    source: String,
    simulate_fail_after_backup: bool,
}

fn parse_package_source_spec(raw: &str) -> PackageSourceSpec {
    const FAIL_AFTER_BACKUP_PREFIX: &str = "mepm-test-fail-after-backup::";
    if let Some(source) = raw.strip_prefix(FAIL_AFTER_BACKUP_PREFIX) {
        return PackageSourceSpec {
            source: source.to_string(),
            simulate_fail_after_backup: true,
        };
    }

    PackageSourceSpec {
        source: raw.to_string(),
        simulate_fail_after_backup: false,
    }
}

fn ensure_supported_package(package: &PlatformPackage) -> Result<()> {
    if package.package_type != "zip"
        && package.package_type != "bundle-dir"
        && package.package_type != "tar.gz"
    {
        bail!("Only zip, tar.gz, and bundle-dir plugin packages are supported in v1");
    }
    if (package.package_type == "zip" || package.package_type == "tar.gz")
        && package.sha256.starts_with("REPLACE_")
    {
        bail!("The manifest for this plugin still contains a placeholder checksum.");
    }
    Ok(())
}

fn stage_bundle_directory(source_bundle: &Path, staged_bundle: &Path) -> Result<()> {
    if staged_bundle.exists() {
        fs::remove_dir_all(staged_bundle)
            .with_context(|| format!("Failed to remove {}", staged_bundle.display()))?;
    }
    let destination_parent = staged_bundle
        .parent()
        .ok_or_else(|| anyhow!("Staging path was missing a parent directory"))?;
    fs::create_dir_all(destination_parent)
        .with_context(|| format!("Failed to create {}", destination_parent.display()))?;

    let mut options = CopyOptions::new();
    options.copy_inside = false;
    options.overwrite = true;
    copy_dir(source_bundle, destination_parent, &options)
        .with_context(|| format!("Failed to stage {}", source_bundle.display()))?;
    Ok(())
}

fn write_bundle_install_stamp(
    bundle_root: &Path,
    plugin_id: &str,
    installed_version: &str,
    bundle_identifier: &str,
    installed_at: &str,
) -> Result<()> {
    let resources_dir = bundle_root.join("Contents").join("Resources");
    fs::create_dir_all(&resources_dir)
        .with_context(|| format!("Failed to create {}", resources_dir.display()))?;
    let stamp = BundleInstallStamp {
        plugin_id: plugin_id.to_string(),
        installed_version: installed_version.to_string(),
        bundle_identifier: bundle_identifier.to_string(),
        installed_at: installed_at.to_string(),
    };
    let raw = serde_json::to_string_pretty(&stamp)?;
    let stamp_path = bundle_stamp_path(bundle_root);
    fs::write(&stamp_path, raw).with_context(|| format!("Failed to write {}", stamp_path.display()))
}

async fn load_package_bytes(source: &str) -> Result<Vec<u8>> {
    if let Ok(local_path) = resolve_local_source_path(source) {
        return fs::read(&local_path)
            .with_context(|| format!("Failed to read {}", local_path.display()));
    }

    let client = Client::builder()
        .user_agent("MoazElgabryPlugins/0.1.0")
        .build()
        .context("Failed to create download client")?;
    let bytes = client
        .get(source)
        .send()
        .await
        .with_context(|| format!("Failed to download {source}"))?
        .error_for_status()
        .with_context(|| format!("Unexpected response while downloading {source}"))?
        .bytes()
        .await
        .context("Failed to read downloaded plugin archive")?;
    Ok(bytes.to_vec())
}

fn ensure_hosts_closed(host_processes: &[String]) -> Result<()> {
    let system = System::new_all();
    let mut running = Vec::new();
    let candidates: Vec<String> = host_processes
        .iter()
        .map(|candidate| normalize_process_name(candidate))
        .collect();

    for process in system.processes().values() {
        let process_name = process.name().to_string_lossy().to_string();
        if process_matches_candidates(&process_name, process.exe(), &candidates) {
            running.push(process_name);
        }
    }

    if running.is_empty() {
        return Ok(());
    }

    running.sort();
    running.dedup();
    bail!(
        "Close the running host applications before installing: {}",
        running.join(", ")
    )
}

fn process_matches_candidates(
    process_name: &str,
    process_exe: Option<&Path>,
    candidates: &[String],
) -> bool {
    let normalized_name = normalize_process_name(process_name);
    let normalized_exe = process_exe
        .and_then(|path| path.file_name())
        .map(|name| normalize_process_name(&name.to_string_lossy()))
        .unwrap_or_default();

    candidates
        .iter()
        .any(|candidate| *candidate == normalized_name || *candidate == normalized_exe)
}

fn normalize_process_name(value: &str) -> String {
    let trimmed = value.trim().to_lowercase();
    let basename = Path::new(&trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&trimmed)
        .to_string();

    basename
        .strip_suffix(".exe")
        .or_else(|| basename.strip_suffix(".app"))
        .unwrap_or(&basename)
        .trim()
        .to_string()
}

fn verify_archive_hash(bytes: &[u8], expected: &str) -> Result<()> {
    let actual = hex::encode(Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(expected) {
        return Ok(());
    }
    bail!(
        "Checksum mismatch. Expected {}, downloaded {}",
        expected,
        actual
    )
}

fn extract_zip(bytes: &[u8], destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("Failed to create {}", destination.display()))?;

    let reader = Cursor::new(bytes.to_vec());
    let mut archive = ZipArchive::new(reader).context("Downloaded archive was not a valid ZIP")?;

    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let relative = match file.enclosed_name() {
            Some(path) => path.to_path_buf(),
            None => continue,
        };
        if should_skip_zip_entry(&relative) {
            continue;
        }
        validate_zip_entry_path(&relative).with_context(|| {
            format!(
                "Archive entry '{}' could not be extracted safely",
                file.name()
            )
        })?;
        let output = destination.join(relative);
        if zip_entry_is_dir(&file) {
            fs::create_dir_all(&output).with_context(|| {
                format!("Failed to create extracted directory {}", output.display())
            })?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory {}", parent.display())
            })?;
        }
        let mut writer = fs::File::create(&output)
            .with_context(|| format!("Failed to create extracted file {}", output.display()))?;
        std::io::copy(&mut file, &mut writer)
            .with_context(|| format!("Failed to write extracted file {}", output.display()))?;
    }

    Ok(())
}

fn zip_entry_is_dir(file: &zip::read::ZipFile<'_>) -> bool {
    file.is_dir() || file.name().ends_with('/') || file.name().ends_with('\\')
}

fn should_skip_zip_entry(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name == "__MACOSX" || name.starts_with("._")
    })
}

fn validate_zip_entry_path(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        for component in path.components() {
            let name = component.as_os_str().to_string_lossy();
            if name.is_empty() {
                continue;
            }
            if name.ends_with(' ') || name.ends_with('.') {
                bail!(
                    "Windows archive entry component '{}' ends with an invalid character",
                    name
                );
            }
            if name
                .chars()
                .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
            {
                bail!(
                    "Windows archive entry component '{}' contains characters invalid on Windows",
                    name
                );
            }
        }
    }

    Ok(())
}

fn extract_tar_gz(bytes: &[u8], destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("Failed to create {}", destination.display()))?;

    let reader = Cursor::new(bytes);
    let decoder = GzDecoder::new(reader);
    let mut archive = Archive::new(decoder);

    for entry in archive
        .entries()
        .context("Downloaded archive was not a valid tar.gz")?
    {
        let mut entry = entry?;
        entry.unpack_in(destination).with_context(|| {
            format!(
                "Failed to extract archive entry into {}",
                destination.display()
            )
        })?;
    }

    Ok(())
}

fn find_bundle_root(root: &Path) -> Result<Option<PathBuf>> {
    for entry in WalkDir::new(root).min_depth(1).max_depth(6) {
        let entry = entry?;
        if entry.file_type().is_dir()
            && entry
                .file_name()
                .to_string_lossy()
                .to_lowercase()
                .ends_with(".ofx.bundle")
        {
            return Ok(Some(entry.into_path()));
        }
    }
    Ok(None)
}

fn verify_bundle(bundle_root: &Path, package: &PlatformPackage) -> Result<()> {
    let name = bundle_root
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .ok_or_else(|| anyhow!("Bundle path was missing a file name"))?;
    if name != package.bundle_name {
        bail!(
            "Downloaded bundle name mismatch. Expected {}, found {}",
            package.bundle_name,
            name
        );
    }

    let plist_path = bundle_root.join("Contents").join("Info.plist");
    if !plist_path.exists() {
        if package.platform == "windows" {
            let expected_binary = package
                .bundle_name
                .strip_suffix(".ofx.bundle")
                .unwrap_or(&package.bundle_name);
            let win64_binary = bundle_root
                .join("Contents")
                .join("Win64")
                .join(format!("{expected_binary}.ofx"));
            if win64_binary.exists() {
                return Ok(());
            }
            bail!("Windows bundle did not contain {}", win64_binary.display());
        }
        if package.platform == "linux" {
            let linux_binary = linux_bundle_binary_path(bundle_root, package)?;
            if linux_binary.exists() {
                return Ok(());
            }
            bail!("Linux bundle did not contain {}", linux_binary.display());
        }
        bail!("Bundle did not contain Contents/Info.plist");
    }
    let plist = PlistValue::from_file(&plist_path)
        .with_context(|| format!("Failed to parse {}", plist_path.display()))?;
    let dictionary = plist
        .as_dictionary()
        .ok_or_else(|| anyhow!("Info.plist root was not a dictionary"))?;

    let bundle_identifier = dictionary
        .get("CFBundleIdentifier")
        .and_then(|value| value.as_string())
        .ok_or_else(|| anyhow!("CFBundleIdentifier was missing from Info.plist"))?;

    if bundle_identifier != package.bundle_identifier {
        bail!(
            "Bundle identifier mismatch. Expected {}, found {}",
            package.bundle_identifier,
            bundle_identifier
        );
    }

    Ok(())
}

fn linux_bundle_binary_path(bundle_root: &Path, package: &PlatformPackage) -> Result<PathBuf> {
    let binary_name = package
        .bundle_name
        .strip_suffix(".ofx.bundle")
        .unwrap_or(&package.bundle_name);
    Ok(bundle_root
        .join("Contents")
        .join(linux_arch_dir())
        .join(format!("{binary_name}.ofx")))
}

fn linux_arch_dir() -> &'static str {
    match current_arch() {
        "x86_64" => "Linux-x86-64",
        "aarch64" => "Linux-aarch64",
        _ => "Linux-x86-64",
    }
}

fn privileged_install_windows(
    source_bundle: &Path,
    install_root: &Path,
    bundle_name: &str,
    simulate_fail_after_backup: bool,
) -> Result<()> {
    let token = format!("{}-{}", std::process::id(), Utc::now().timestamp_millis());
    let script_dir = std::env::temp_dir().join("Moaz Elgabry Plugins");
    fs::create_dir_all(&script_dir)
        .with_context(|| format!("Failed to create {}", script_dir.display()))?;
    let script_path = script_dir.join(format!("install-plugin-{token}.ps1"));
    let log_path = script_dir.join(format!("install-plugin-{token}.log"));
    let script = format!(
        r#"$ErrorActionPreference = "Stop"
$SourceBundle = '{source}'
$InstallRoot = '{install_root}'
$BundleName = '{bundle_name}'
$LogPath = '{log}'
$SimulateFailureAfterBackup = {simulate_fail}
$target = Join-Path $InstallRoot $BundleName
$backup = Join-Path $InstallRoot ($BundleName + ".manager-backup")

function Write-InstallLog([string]$Message) {{
  $timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
  Add-Content -Path $LogPath -Value "[$timestamp] $Message"
}}

try {{
  Set-Content -Path $LogPath -Value ""
  Write-InstallLog "Starting Windows plugin install"
  Write-InstallLog "SourceBundle=$SourceBundle"
  Write-InstallLog "InstallRoot=$InstallRoot"
  Write-InstallLog "BundleName=$BundleName"

  if (Test-Path $backup) {{ Remove-Item $backup -Recurse -Force }}
  if (!(Test-Path $InstallRoot)) {{ New-Item -ItemType Directory -Path $InstallRoot | Out-Null }}
  if (Test-Path $target) {{
    Write-InstallLog "Moving existing bundle to backup"
    Move-Item -LiteralPath $target -Destination $backup -Force
  }}
  if ($SimulateFailureAfterBackup -eq 1) {{
    Write-InstallLog "Simulating failure after backup for rollback test"
    throw "Simulated install failure after backup"
  }}
  Write-InstallLog "Copying new bundle"
  Copy-Item -LiteralPath $SourceBundle -Destination $InstallRoot -Recurse -Force
  if (Test-Path $backup) {{ Remove-Item $backup -Recurse -Force }}
  Write-InstallLog "Install completed successfully"
  exit 0
}}
catch {{
  Write-InstallLog ("ERROR: " + $_.Exception.Message)
  if ($_.ScriptStackTrace) {{
    Write-InstallLog ("STACK: " + $_.ScriptStackTrace)
  }}
  if (Test-Path $target) {{ Remove-Item $target -Recurse -Force }}
  if (Test-Path $backup) {{ Move-Item -LiteralPath $backup -Destination $target -Force }}
  throw
}}
"#,
        source = escape_ps(&source_bundle.display().to_string()),
        install_root = escape_ps(&install_root.display().to_string()),
        bundle_name = escape_ps(bundle_name),
        log = escape_ps(&log_path.display().to_string()),
        simulate_fail = if simulate_fail_after_backup { 1 } else { 0 }
    );
    fs::write(&script_path, script)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;

    let outer_command = format!(
        "$ErrorActionPreference='Stop'; Set-Content -Path '{}' -Value ''; Add-Content -Path '{}' -Value '[outer] launching elevated installer'; try {{ $ps = Join-Path $env:SystemRoot 'System32\\WindowsPowerShell\\v1.0\\powershell.exe'; $argList = '-NoProfile -ExecutionPolicy Bypass -File \"{}\"'; Add-Content -Path '{}' -Value ('[outer] command: ' + $ps + ' ' + $argList); $p = Start-Process -FilePath $ps -Verb RunAs -WindowStyle Hidden -Wait -PassThru -ArgumentList $argList; Add-Content -Path '{}' -Value ('[outer] elevated installer exit code: ' + $p.ExitCode); exit $p.ExitCode }} catch {{ Add-Content -Path '{}' -Value ('[outer] ERROR: ' + $_.Exception.Message); if ($_.ScriptStackTrace) {{ Add-Content -Path '{}' -Value ('[outer] STACK: ' + $_.ScriptStackTrace) }} exit 1 }}",
        escape_ps(&log_path.display().to_string()),
        escape_ps(&log_path.display().to_string()),
        escape_ps(&script_path.display().to_string()),
        escape_ps(&log_path.display().to_string()),
        escape_ps(&log_path.display().to_string()),
        escape_ps(&log_path.display().to_string()),
        escape_ps(&log_path.display().to_string())
    );

    let mut command = Command::new(windows_powershell_exe());
    command.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &outer_command,
    ]);
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    let status = command
        .status()
        .context("Failed to start elevated PowerShell installer")?;

    if status.success() {
        let _ = fs::remove_file(&script_path);
        let _ = fs::remove_file(&log_path);
        return Ok(());
    }

    let details = fs::read_to_string(&log_path).unwrap_or_default();
    let _ = fs::remove_file(&script_path);
    if details.trim().is_empty() {
        bail!(
            "Windows installation was cancelled or failed with exit code {:?}. Log file: {}",
            status.code(),
            log_path.display()
        );
    }

    let inner_started = details.contains("Starting Windows plugin install");
    if !inner_started {
        let exit_code = status.code();
        bail!(
            "Windows installation could not start with administrator privileges. Please accept the Windows admin prompt and try again. Exit code {:?}. Log file: {}. Details: {}",
            exit_code,
            log_path.display(),
            details.trim()
        );
    }

    bail!(
        "Windows installation failed with exit code {:?}. Log file: {}. Details: {}",
        status.code(),
        log_path.display(),
        details.trim()
    )
}

fn privileged_uninstall_windows(target_bundle: &Path, _bundle_name: &str) -> Result<()> {
    let token = format!("{}-{}", std::process::id(), Utc::now().timestamp_millis());
    let script_dir = std::env::temp_dir().join("Moaz Elgabry Plugins");
    fs::create_dir_all(&script_dir)
        .with_context(|| format!("Failed to create {}", script_dir.display()))?;
    let script_path = script_dir.join(format!("uninstall-plugin-{token}.ps1"));
    let log_path = script_dir.join(format!("uninstall-plugin-{token}.log"));
    let script = format!(
        r#"$ErrorActionPreference = "Stop"
$TargetBundle = '{target}'
$LogPath = '{log}'
$StampPath = Join-Path $TargetBundle 'Contents\Resources\moaz-elgabry-plugins.install.json'
$LegacyStampPath = Join-Path $TargetBundle 'Contents\Resources\moazelgabry-plugin-manager.install.json'

function Write-UninstallLog([string]$Message) {{
  $timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
  Add-Content -Path $LogPath -Value "[$timestamp] $Message"
}}

Set-Content -Path $LogPath -Value ""
Write-UninstallLog "Starting Windows plugin uninstall"
Write-UninstallLog "TargetBundle=$TargetBundle"

if (!(Test-Path $TargetBundle)) {{
  Write-UninstallLog "Target bundle already missing"
  exit 0
}}

if (Test-Path $StampPath) {{ Remove-Item $StampPath -Force }}
if (Test-Path $LegacyStampPath) {{ Remove-Item $LegacyStampPath -Force }}
Remove-Item -LiteralPath $TargetBundle -Recurse -Force
Write-UninstallLog "Uninstall completed successfully"
"#,
        target = escape_ps(&target_bundle.display().to_string()),
        log = escape_ps(&log_path.display().to_string())
    );
    fs::write(&script_path, script)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;

    let outer_command = format!(
        "$ErrorActionPreference='Stop'; Set-Content -Path '{}' -Value ''; Add-Content -Path '{}' -Value '[outer] launching elevated uninstaller'; try {{ $ps = Join-Path $env:SystemRoot 'System32\\WindowsPowerShell\\v1.0\\powershell.exe'; $argList = '-NoProfile -ExecutionPolicy Bypass -File \"{}\"'; Add-Content -Path '{}' -Value ('[outer] command: ' + $ps + ' ' + $argList); $p = Start-Process -FilePath $ps -Verb RunAs -WindowStyle Hidden -Wait -PassThru -ArgumentList $argList; Add-Content -Path '{}' -Value ('[outer] elevated uninstaller exit code: ' + $p.ExitCode); exit $p.ExitCode }} catch {{ Add-Content -Path '{}' -Value ('[outer] ERROR: ' + $_.Exception.Message); if ($_.ScriptStackTrace) {{ Add-Content -Path '{}' -Value ('[outer] STACK: ' + $_.ScriptStackTrace) }} exit 1 }}",
        escape_ps(&log_path.display().to_string()),
        escape_ps(&log_path.display().to_string()),
        escape_ps(&script_path.display().to_string()),
        escape_ps(&log_path.display().to_string()),
        escape_ps(&log_path.display().to_string()),
        escape_ps(&log_path.display().to_string()),
        escape_ps(&log_path.display().to_string())
    );

    let mut command = Command::new(windows_powershell_exe());
    command.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &outer_command,
    ]);
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);
    let status = command
        .status()
        .context("Failed to start elevated PowerShell uninstaller")?;

    if status.success() {
        let _ = fs::remove_file(&script_path);
        let _ = fs::remove_file(&log_path);
        return Ok(());
    }

    let details = fs::read_to_string(&log_path).unwrap_or_default();
    let _ = fs::remove_file(&script_path);
    if details.trim().is_empty() {
        bail!(
            "Windows uninstall was cancelled or failed with exit code {:?}. Log file: {}",
            status.code(),
            log_path.display()
        );
    }

    let inner_started = details.contains("Starting Windows plugin uninstall");
    if !inner_started {
        bail!(
            "Windows uninstall could not start with administrator privileges. Please accept the Windows admin prompt and try again. Exit code {:?}. Log file: {}. Details: {}",
            status.code(),
            log_path.display(),
            details.trim()
        );
    }

    bail!(
        "Windows uninstall failed with exit code {:?}. Log file: {}. Details: {}",
        status.code(),
        log_path.display(),
        details.trim()
    )
}

fn privileged_install_macos(
    source_bundle: &Path,
    install_root: &Path,
    bundle_name: &str,
    simulate_fail_after_backup: bool,
) -> Result<()> {
    let script_dir = tempdir().context("Failed to create temp directory for installer script")?;
    let script_path = script_dir.path().join("install-plugin.sh");
    let script = format!(
        r#"#!/bin/sh
set -e

SOURCE_BUNDLE="$1"
INSTALL_ROOT="$2"
BUNDLE_NAME="$3"
TARGET="$INSTALL_ROOT/$BUNDLE_NAME"
BACKUP="$INSTALL_ROOT/$BUNDLE_NAME.manager-backup"

cleanup_on_error() {{
  status=$?
  if [ "$status" -ne 0 ]; then
    rm -rf "$TARGET"
    if [ -d "$BACKUP" ]; then
      mv "$BACKUP" "$TARGET"
    fi
  fi
  exit "$status"
}}

trap cleanup_on_error EXIT

mkdir -p "$INSTALL_ROOT"
rm -rf "$BACKUP"
if [ -d "$TARGET" ]; then
  mv "$TARGET" "$BACKUP"
fi
if [ "{simulate_fail}" = "1" ]; then
  exit 91
fi
cp -R "$SOURCE_BUNDLE" "$INSTALL_ROOT/"
chmod -R 755 "$TARGET"
chown -R root:wheel "$TARGET"
xattr -dr com.apple.quarantine "$TARGET" || true
codesign --force --deep --sign - "$TARGET"
rm -rf "$BACKUP"
trap - EXIT
"#,
        simulate_fail = if simulate_fail_after_backup { "1" } else { "0" }
    );
    fs::write(&script_path, script)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&script_path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions)?;
    }

    let escaped_script = escape_osascript(&script_path.display().to_string());
    let escaped_source = escape_osascript(&source_bundle.display().to_string());
    let escaped_root = escape_osascript(&install_root.display().to_string());
    let escaped_bundle = escape_osascript(bundle_name);

    let status = Command::new("osascript")
        .args([
            "-e",
            &format!(r#"set scriptPath to "{}""#, escaped_script),
            "-e",
            &format!(r#"set sourceBundle to "{}""#, escaped_source),
            "-e",
            &format!(r#"set installRoot to "{}""#, escaped_root),
            "-e",
            &format!(r#"set bundleName to "{}""#, escaped_bundle),
            "-e",
            r#"do shell script quoted form of scriptPath & " " & quoted form of sourceBundle & " " & quoted form of installRoot & " " & quoted form of bundleName with administrator privileges"#,
        ])
        .status()
        .context("Failed to start elevated macOS installer")?;

    if status.success() {
        return Ok(());
    }

    bail!(
        "macOS installation was cancelled or failed with exit code {:?}",
        status.code()
    )
}

fn privileged_uninstall_macos(target_bundle: &Path, bundle_name: &str) -> Result<()> {
    let script_dir = tempdir().context("Failed to create temp directory for uninstaller script")?;
    let script_path = script_dir.path().join("uninstall-plugin.sh");
    let script = format!(
        r#"#!/bin/sh
set -e

TARGET_BUNDLE="$1"
BUNDLE_NAME="$2"

if [ ! -d "$TARGET_BUNDLE" ]; then
  exit 0
fi

rm -f "$TARGET_BUNDLE/Contents/Resources/moaz-elgabry-plugins.install.json"
rm -f "$TARGET_BUNDLE/Contents/Resources/moazelgabry-plugin-manager.install.json"
rm -rf "$TARGET_BUNDLE"
"#
    );
    fs::write(&script_path, script)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&script_path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions)?;
    }

    let escaped_script = escape_osascript(&script_path.display().to_string());
    let escaped_target = escape_osascript(&target_bundle.display().to_string());
    let escaped_bundle = escape_osascript(bundle_name);

    let status = Command::new("osascript")
        .args([
            "-e",
            &format!(r#"set scriptPath to "{}""#, escaped_script),
            "-e",
            &format!(r#"set targetBundle to "{}""#, escaped_target),
            "-e",
            &format!(r#"set bundleName to "{}""#, escaped_bundle),
            "-e",
            r#"do shell script quoted form of scriptPath & " " & quoted form of targetBundle & " " & quoted form of bundleName with administrator privileges"#,
        ])
        .status()
        .context("Failed to start elevated macOS uninstaller")?;

    if status.success() {
        return Ok(());
    }

    bail!(
        "macOS uninstall was cancelled or failed with exit code {:?}",
        status.code()
    )
}

fn privileged_install_linux(
    source_bundle: &Path,
    install_root: &Path,
    bundle_name: &str,
    simulate_fail_after_backup: bool,
) -> Result<()> {
    let pkexec = find_linux_pkexec().ok_or_else(|| {
        anyhow!("pkexec executable was not found. A PolicyKit-capable environment is required.")
    })?;
    let script_dir =
        tempdir().context("Failed to create temp directory for Linux installer script")?;
    let script_path = script_dir.path().join("install-plugin.sh");
    let log_path = std::env::temp_dir()
        .join("Moaz Elgabry Plugins")
        .join(format!(
            "install-plugin-{}-{}.log",
            std::process::id(),
            Utc::now().timestamp_millis()
        ));
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let script = format!(
        r#"#!/bin/sh
set -eu

SOURCE_BUNDLE="$1"
INSTALL_ROOT="$2"
BUNDLE_NAME="$3"
LOG_PATH="$4"
SIMULATE_FAIL="$5"
TARGET="$INSTALL_ROOT/$BUNDLE_NAME"
BACKUP="$INSTALL_ROOT/$BUNDLE_NAME.manager-backup"

write_log() {{
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$1" >> "$LOG_PATH"
}}

cleanup_on_error() {{
  status=$?
  if [ "$status" -ne 0 ]; then
    write_log "ERROR: install failed with exit code $status"
    rm -rf "$TARGET"
    if [ -d "$BACKUP" ]; then
      mv "$BACKUP" "$TARGET"
      write_log "Restored previous bundle from backup"
    fi
  fi
  exit "$status"
}}

trap cleanup_on_error EXIT
: > "$LOG_PATH"
write_log "Starting Linux plugin install"
write_log "SourceBundle=$SOURCE_BUNDLE"
write_log "InstallRoot=$INSTALL_ROOT"
write_log "BundleName=$BUNDLE_NAME"

mkdir -p "$INSTALL_ROOT"
rm -rf "$BACKUP"
if [ -d "$TARGET" ]; then
  write_log "Moving existing bundle to backup"
  mv "$TARGET" "$BACKUP"
fi
if [ "$SIMULATE_FAIL" = "1" ]; then
  write_log "Simulating failure after backup for rollback test"
  exit 91
fi
write_log "Copying new bundle"
cp -R "$SOURCE_BUNDLE" "$INSTALL_ROOT/"
chmod -R 755 "$TARGET"
chown -R root:root "$TARGET"
rm -rf "$BACKUP"
write_log "Install completed successfully"
trap - EXIT
"#,
    );
    fs::write(&script_path, script)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&script_path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions)?;
    }

    let status = Command::new(pkexec)
        .arg(&script_path)
        .arg(source_bundle)
        .arg(install_root)
        .arg(bundle_name)
        .arg(&log_path)
        .arg(if simulate_fail_after_backup { "1" } else { "0" })
        .status()
        .context("Failed to start elevated Linux installer")?;

    if status.success() {
        let _ = fs::remove_file(&log_path);
        return Ok(());
    }

    let details = fs::read_to_string(&log_path).unwrap_or_default();
    if details.trim().is_empty() {
        bail!(
            "Linux installation was cancelled or failed with exit code {:?}. Log file: {}",
            status.code(),
            log_path.display()
        );
    }

    bail!(
        "Linux installation failed with exit code {:?}. Log file: {}. Details: {}",
        status.code(),
        log_path.display(),
        details.trim()
    )
}

fn privileged_uninstall_linux(target_bundle: &Path, bundle_name: &str) -> Result<()> {
    let pkexec = find_linux_pkexec().ok_or_else(|| {
        anyhow!("pkexec executable was not found. A PolicyKit-capable environment is required.")
    })?;
    let script_dir =
        tempdir().context("Failed to create temp directory for Linux uninstaller script")?;
    let script_path = script_dir.path().join("uninstall-plugin.sh");
    let log_path = std::env::temp_dir()
        .join("Moaz Elgabry Plugins")
        .join(format!(
            "uninstall-plugin-{}-{}.log",
            std::process::id(),
            Utc::now().timestamp_millis()
        ));
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let script = r#"#!/bin/sh
set -eu

TARGET_BUNDLE="$1"
LOG_PATH="$2"
BUNDLE_NAME="$3"

write_log() {
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$1" >> "$LOG_PATH"
}

: > "$LOG_PATH"
write_log "Starting Linux plugin uninstall"
write_log "TargetBundle=$TARGET_BUNDLE"

if [ ! -d "$TARGET_BUNDLE" ]; then
  write_log "Target bundle already missing"
  exit 0
fi

rm -f "$TARGET_BUNDLE/Contents/Resources/moaz-elgabry-plugins.install.json"
rm -f "$TARGET_BUNDLE/Contents/Resources/moazelgabry-plugin-manager.install.json"
rm -rf "$TARGET_BUNDLE"
write_log "Uninstall completed successfully"
"#;
    fs::write(&script_path, script)
        .with_context(|| format!("Failed to write {}", script_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&script_path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions)?;
    }

    let status = Command::new(pkexec)
        .arg(&script_path)
        .arg(target_bundle)
        .arg(&log_path)
        .arg(bundle_name)
        .status()
        .context("Failed to start elevated Linux uninstaller")?;

    if status.success() {
        let _ = fs::remove_file(&log_path);
        return Ok(());
    }

    let details = fs::read_to_string(&log_path).unwrap_or_default();
    if details.trim().is_empty() {
        bail!(
            "Linux uninstall was cancelled or failed with exit code {:?}. Log file: {}",
            status.code(),
            log_path.display()
        );
    }

    bail!(
        "Linux uninstall failed with exit code {:?}. Log file: {}. Details: {}",
        status.code(),
        log_path.display(),
        details.trim()
    )
}

fn find_linux_pkexec() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }

    let candidates = ["/usr/bin/pkexec", "/bin/pkexec", "/usr/local/bin/pkexec"];
    for candidate in candidates {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn windows_powershell_exe() -> PathBuf {
    for variable in ["SystemRoot", "WINDIR"] {
        if let Some(root) = std::env::var_os(variable) {
            let candidate = PathBuf::from(root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe");
            if candidate.exists() {
                return candidate;
            }
        }
    }

    let default = PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
    if default.exists() {
        return default;
    }

    PathBuf::from("powershell.exe")
}

fn escape_ps(raw: &str) -> String {
    raw.replace('\'', "''")
}

fn escape_osascript(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

fn bundle_stamp_path(bundle_root: &Path) -> PathBuf {
    bundle_root
        .join("Contents")
        .join("Resources")
        .join("moaz-elgabry-plugins.install.json")
}

fn legacy_bundle_stamp_path(bundle_root: &Path) -> PathBuf {
    bundle_root
        .join("Contents")
        .join("Resources")
        .join("moazelgabry-plugin-manager.install.json")
}

fn resolve_local_source_path(raw: &str) -> Result<PathBuf> {
    let path = if let Some(stripped) = raw.strip_prefix("file:///") {
        normalize_file_uri_path(stripped)
    } else {
        normalize_runtime_path(raw)
    };

    if path.exists() {
        return Ok(path);
    }

    bail!("Local package source was not found at {}", path.display())
}

fn normalize_file_uri_path(raw: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(raw.replace('/', "\\"))
    } else {
        PathBuf::from(format!("/{}", raw.trim_start_matches('/')))
    }
}

fn normalize_runtime_path(raw: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(raw.replace('/', "\\"))
    } else {
        PathBuf::from(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_environment_replaces_supported_tokens() {
        let diagnostics = PluginDiagnostics {
            enabled: true,
            log_source_path: Default::default(),
            environment: [
                (
                    "LENSDIFF_LOG_DIR".to_string(),
                    "{logSourcePath}".to_string(),
                ),
                (
                    "LENSDIFF_EXPORT_DIR".to_string(),
                    "{destinationDir}".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let log_source = PathBuf::from("C:\\Logs");
        let destination = PathBuf::from("D:\\Exports");
        let entries = diagnostics_environment(&diagnostics, &log_source, &destination).unwrap();

        assert!(entries.contains(&(
            "LENSDIFF_LOG_DIR".to_string(),
            log_source.display().to_string()
        )));
        assert!(entries.contains(&(
            "LENSDIFF_EXPORT_DIR".to_string(),
            destination.display().to_string()
        )));
    }

    #[test]
    fn diagnostics_environment_rejects_invalid_keys() {
        let diagnostics = PluginDiagnostics {
            enabled: true,
            log_source_path: Default::default(),
            environment: [("1BAD".to_string(), "value".to_string())]
                .into_iter()
                .collect(),
        };

        assert!(diagnostics_environment(
            &diagnostics,
            &PathBuf::from("logs"),
            &PathBuf::from("exports")
        )
        .is_err());
    }

    #[test]
    fn expand_diagnostics_path_handles_home_prefix() {
        let expanded = expand_diagnostics_path("~/MoazElgabry/LensDiff/logs").unwrap();
        assert!(expanded.ends_with(Path::new("MoazElgabry").join("LensDiff").join("logs")));
    }

    #[test]
    fn process_matching_does_not_treat_proxy_resolver_as_resolve() {
        let candidates = ["Resolve", "Resolve.exe", "DaVinci Resolve"]
            .into_iter()
            .map(normalize_process_name)
            .collect::<Vec<_>>();

        assert!(!process_matches_candidates(
            "Proxy Resolver",
            Some(Path::new("/usr/lib/proxy-resolver")),
            &candidates
        ));
        assert!(process_matches_candidates(
            "resolve",
            Some(Path::new("/opt/resolve/bin/resolve")),
            &candidates
        ));
        assert!(process_matches_candidates(
            "DaVinci Resolve",
            Some(Path::new(
                "/Applications/DaVinci Resolve/DaVinci Resolve.app"
            )),
            &candidates
        ));
    }

    #[test]
    fn copy_log_sources_copies_multiple_files_and_directories() {
        let temp = tempdir().unwrap();
        let source_root = temp.path().join("sources");
        let source_dir = source_root.join("folder");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(source_dir.join("inside.log"), "folder log").unwrap();
        let source_file = source_root.join("single.log");
        fs::write(&source_file, "single log").unwrap();
        let destination = temp.path().join("export");

        let outcome = copy_log_sources_to_export_or_notice(
            &[source_dir, source_file],
            &destination,
            "Test Plugin",
            &[],
        )
        .unwrap();

        assert_eq!(outcome.copied_files, 2);
        assert!(!outcome.notice_written);
        assert_eq!(
            fs::read_to_string(destination.join("inside.log")).unwrap(),
            "folder log"
        );
        assert_eq!(
            fs::read_to_string(destination.join("single.log")).unwrap(),
            "single log"
        );
    }

    #[test]
    fn copy_log_sources_writes_note_when_all_sources_are_missing() {
        let temp = tempdir().unwrap();
        let destination = temp.path().join("export");

        let outcome = copy_log_sources_to_export_or_notice(
            &[
                temp.path().join("missing-one.log"),
                temp.path().join("missing-two.log"),
            ],
            &destination,
            "Test Plugin",
            &[("PLUGIN_LOG".to_string(), "1".to_string())],
        )
        .unwrap();

        assert_eq!(outcome.copied_files, 0);
        assert!(outcome.notice_written);
        let note = fs::read_to_string(destination.join("diagnostics-export-note.txt")).unwrap();
        assert!(note.contains("missing-one.log"));
        assert!(note.contains("missing-two.log"));
        assert!(note.contains("PLUGIN_LOG=1"));
    }

    #[test]
    fn clear_log_sources_removes_files_and_directory_contents() {
        let temp = tempdir().unwrap();
        let log_file = temp.path().join("single.log");
        fs::write(&log_file, "old file log").unwrap();
        let log_dir = temp.path().join("PluginLogs");
        fs::create_dir_all(log_dir.join("nested")).unwrap();
        fs::write(log_dir.join("old.log"), "old folder log").unwrap();
        fs::write(log_dir.join("nested").join("old.log"), "old nested log").unwrap();

        clear_log_sources(&[log_file.clone(), log_dir.clone()]).unwrap();

        assert!(!log_file.exists());
        assert!(log_dir.exists());
        assert!(fs::read_dir(&log_dir).unwrap().next().is_none());
    }

    #[test]
    fn clear_log_sources_refuses_broad_temp_root() {
        assert!(clear_log_sources(&[std::env::temp_dir()]).is_err());
    }
}
