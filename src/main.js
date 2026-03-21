import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";
import chromaspaceIcon from "./assets/plugin-icons/chromaspace.png";
import meOpenDRTIcon from "./assets/plugin-icons/me-opendrt.png";
import "./styles.css";

const state = {
  busy: false,
  dashboard: null,
  activeOperation: null
};

const elements = {
  version: document.querySelector("#manager-version"),
  platform: document.querySelector("#manager-platform"),
  catalogSource: document.querySelector("#catalog-source"),
  updaterStatus: document.querySelector("#updater-status"),
  refreshButton: document.querySelector("#refresh-button"),
  updateButton: document.querySelector("#check-updates-button"),
  pluginList: document.querySelector("#plugin-list"),
  activityLog: document.querySelector("#activity-log"),
  alertBanner: document.querySelector("#alert-banner"),
  alertSummary: document.querySelector("#alert-summary"),
  alertMessage: document.querySelector("#alert-message"),
  alertDetails: document.querySelector("#alert-details"),
  alertDismiss: document.querySelector("#alert-dismiss")
};

function logActivity(message) {
  const item = document.createElement("div");
  item.className = "activity-item";
  item.innerHTML = `<time>${new Date().toLocaleString()}</time><div>${message}</div>`;
  elements.activityLog.prepend(item);
}

function setBusy(nextBusy) {
  state.busy = nextBusy;
  document.querySelectorAll("button").forEach((button) => {
    button.disabled = nextBusy;
  });
}

function operationSteps(kind) {
  if (kind === "catalog") {
    return ["Connecting to catalog", "Loading manifests", "Refreshing plugin status"];
  }
  if (kind === "manager-update") {
    return ["Checking for updates", "Downloading manager update", "Installing manager update"];
  }
  if (kind === "plugin-uninstall") {
    return ["Preparing uninstall", "Removing installed bundle", "Cleaning manager records", "Refreshing plugin status"];
  }
  return ["Preparing package", "Downloading package", "Installing plugin", "Refreshing plugin status"];
}

function startOperation(kind, pluginId = null, label = "Working") {
  const steps = operationSteps(kind);
  state.activeOperation = {
    kind,
    pluginId,
    label,
    steps,
    stepIndex: 0
  };

  state.activeOperation.timer = window.setInterval(() => {
    if (!state.activeOperation || state.activeOperation.kind !== kind || state.activeOperation.pluginId !== pluginId) {
      return;
    }
    const lastStep = state.activeOperation.steps.length - 1;
    state.activeOperation.stepIndex = Math.min(state.activeOperation.stepIndex + 1, lastStep);
    renderPlugins();
  }, 1400);

  renderPlugins();
}

function finishOperation() {
  if (state.activeOperation?.timer) {
    window.clearInterval(state.activeOperation.timer);
  }
  state.activeOperation = null;
  renderPlugins();
}

function parseUiError(error, fallbackSummary = "The operation failed.") {
  const raw = typeof error === "string" ? error : String(error);

  try {
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed.summary === "string") {
      return {
        summary: parsed.summary,
        details: typeof parsed.details === "string" ? parsed.details : raw,
        code: parsed.code ?? "unknown"
      };
    }
  } catch {
    // Some command failures still arrive as plain strings.
  }

  return {
    summary: fallbackSummary,
    details: raw,
    code: "plain_error"
  };
}

function showAlert(errorLike, fallbackSummary) {
  const payload =
    typeof errorLike === "object" && errorLike?.summary
      ? errorLike
      : parseUiError(errorLike, fallbackSummary);

  elements.alertSummary.textContent = payload.summary;
  const hasDetails = Boolean(payload.details) && payload.details !== payload.summary;
  elements.alertMessage.textContent = payload.details ?? "";
  elements.alertDetails.classList.toggle("hidden", !hasDetails);
  elements.alertDetails.open = false;
  elements.alertBanner.classList.remove("hidden");
}

function hideAlert() {
  elements.alertBanner.classList.add("hidden");
  elements.alertSummary.textContent = "";
  elements.alertMessage.textContent = "";
  elements.alertDetails.classList.add("hidden");
  elements.alertDetails.open = false;
}

function statusClass(status) {
  if (status === "Installed" || status === "Up to date") return "ok";
  if (status === "Update available" || status === "Unmanaged install") return "warn";
  return "bad";
}

function actionLabel(plugin) {
  if (!plugin.installed) return "Install";
  if (plugin.needsUpdate) return "Update";
  return "Reinstall";
}

function rollbackButtonLabel(plugin, selectedVersion) {
  if (!selectedVersion) return "Install selected";
  const selected = plugin.availableVersions?.find((option) => option.version === selectedVersion);
  return selected?.actionLabel ?? "Install selected";
}

function selectedVersionHint(plugin, selectedVersion) {
  const selected = plugin.availableVersions?.find((option) => option.version === selectedVersion);
  if (!selected) return "Choose a version to install for this plugin.";

  if (!plugin.installedVersion) {
    return selected.isCurrentLatest
      ? `This installs the latest available release from ${selected.releaseDate}.`
      : `This installs ${selected.version} from ${selected.releaseDate} for project compatibility.`;
  }

  if (selected.version === plugin.installedVersion) {
    return `This reinstalls the currently detected version (${selected.version}).`;
  }

  if (selected.isCurrentLatest) {
    return `This updates ${plugin.displayName} from ${plugin.installedVersion} to the latest release (${selected.version}).`;
  }

  return `This rolls ${plugin.displayName} back from ${plugin.installedVersion} to ${selected.version}.`;
}

function versionDrawerPreview(plugin, selectedVersion) {
  const selected = plugin.availableVersions?.find((option) => option.version === selectedVersion);
  if (!selected) {
    return "Choose a compatible version";
  }

  if (selected.version === plugin.installedVersion) {
    return `Current selection: ${selected.version}`;
  }

  if (selected.isCurrentLatest) {
    return `Latest release: ${selected.version}`;
  }

  return `Project compatibility: ${selected.version}`;
}

function cardToneClass(plugin) {
  if (!plugin.installed) return "pending";
  if (plugin.needsUpdate) return "warn";
  if (plugin.managedInstall) return "ok";
  return "neutral";
}

function primaryActionClass(label) {
  if (label === "Install") return "primary plugin-primary-action plugin-install-action";
  if (label === "Update") return "primary plugin-primary-action plugin-update-action";
  return label === "Reinstall" ? "plugin-secondary-action" : "primary plugin-primary-action";
}

function actionHelperText(primaryLabel) {
  if (primaryLabel === "Update") return "Install the latest release.";
  if (primaryLabel === "Reinstall") return "Reinstall the current version.";
  return "";
}

function pluginOperationMarkup(plugin) {
  const operation = state.activeOperation;
  if (!operation || operation.pluginId !== plugin.pluginId) return "";

  const step = operation.steps[operation.stepIndex] ?? operation.label;
  return `
    <div class="plugin-progress" role="status" aria-live="polite">
      <div class="plugin-progress-copy">
        <p class="plugin-progress-label">${operation.label}</p>
        <p class="plugin-progress-step">${step}</p>
      </div>
      <div class="plugin-progress-bar" aria-hidden="true">
        <span class="plugin-progress-fill"></span>
      </div>
    </div>
  `;
}

function uninstallButtonLabel(plugin) {
  return plugin.managedInstall ? "Uninstall plugin" : "Force uninstall";
}

function uninstallConfirmationMessage(plugin) {
  const intro = plugin.managedInstall
    ? `Uninstall ${plugin.displayName}?`
    : `Force uninstall ${plugin.displayName}?`;
  const warning = plugin.managedInstall
    ? "This removes the installed OFX plugin from the system-wide plugin folder."
    : "This install was not created by the manager. Force uninstall will still remove the detected OFX plugin from the system-wide plugin folder.";
  return `${intro}\n\n${warning}`;
}

function renderMaintenanceDrawer(plugin) {
  if (!plugin.installed) return null;

  const wrapper = document.createElement("details");
  wrapper.className = "maintenance-drawer";
  wrapper.innerHTML = `
    <summary class="maintenance-toggle">
      <div class="maintenance-copy">
        <p class="eyebrow">Maintenance</p>
        <p class="maintenance-title">Uninstall plugin</p>
      </div>
      <span class="maintenance-icon" aria-hidden="true"></span>
    </summary>
    <div class="maintenance-tools">
      <button class="danger-button" data-plugin-id="${plugin.pluginId}" data-action="${plugin.managedInstall ? "uninstall" : "force-uninstall"}">${uninstallButtonLabel(plugin)}</button>
      ${
        plugin.managedInstall
          ? ""
          : '<p class="maintenance-note">Use this only if you want the manager to remove a detected install it did not create.</p>'
      }
    </div>
  `;

  const button = wrapper.querySelector("button");
  button.addEventListener("click", async () => {
    const confirmed = window.confirm(uninstallConfirmationMessage(plugin));
    if (!confirmed) return;
    await applyPluginAction(plugin.pluginId, plugin.managedInstall ? "uninstall" : "force-uninstall");
  });

  return wrapper;
}

function pluginIconMarkup(plugin) {
  if (plugin.pluginId === "chromaspace") {
    return `
      <div class="plugin-icon" aria-hidden="true">
        <img src="${chromaspaceIcon}" alt="" loading="lazy" />
      </div>
    `;
  }

  if (plugin.pluginId === "me-opendrt") {
    return `
      <div class="plugin-icon" aria-hidden="true">
        <img src="${meOpenDRTIcon}" alt="" loading="lazy" />
      </div>
    `;
  }

  return `
    <div class="plugin-icon plugin-icon-fallback" aria-hidden="true">
      <span>${plugin.displayName.charAt(0)}</span>
    </div>
  `;
}

function renderVersionDrawer(plugin) {
  const initialVersion = plugin.installedVersion ?? plugin.availableVersions[0]?.version ?? "";
  const wrapper = document.createElement("details");
  wrapper.className = "version-drawer";
  wrapper.innerHTML = `
    <summary class="version-drawer-toggle">
      <div class="version-drawer-copy">
        <p class="eyebrow">Version history</p>
        <p class="version-drawer-title">Older versions and rollback</p>
      </div>
      <span class="version-drawer-icon" aria-hidden="true"></span>
    </summary>
    <div class="version-tools">
      <div class="version-tools-copy">
        <p class="eyebrow">Selective version install</p>
        <p class="subtle">Choose a different version when a project needs an older match.</p>
      </div>
      <div class="version-picker-row">
        <label class="version-picker">
          <span>Choose a version</span>
          <select data-plugin-id="${plugin.pluginId}">
            ${plugin.availableVersions
              .map(
                (option) =>
                  `<option value="${option.version}" ${option.version === initialVersion ? "selected" : ""}>${option.label} - ${option.releaseDate}</option>`
              )
              .join("")}
          </select>
        </label>
        <button data-plugin-id="${plugin.pluginId}" data-action="install-selected">Install selected</button>
      </div>
      <p class="version-hint"></p>
    </div>
  `;

  const select = wrapper.querySelector("select");
  const installSelectedButton = wrapper.querySelector("button");
  const hint = wrapper.querySelector(".version-hint");

  const refreshCopy = () => {
    installSelectedButton.textContent = rollbackButtonLabel(plugin, select.value);
    hint.textContent = selectedVersionHint(plugin, select.value);
  };

  refreshCopy();

  select.addEventListener("change", refreshCopy);
  installSelectedButton.addEventListener("click", async () => {
    await applyPluginAction(plugin.pluginId, "install-selected", select.value);
  });

  return wrapper;
}

function renderPlugins() {
  const plugins = state.dashboard?.plugins ?? [];

  if (!plugins.length) {
    elements.pluginList.innerHTML = `<div class="empty-state">No plugin manifests are currently available.</div>`;
    return;
  }

  elements.pluginList.innerHTML = "";

  for (const plugin of plugins) {
    const card = document.createElement("article");
    card.className = `plugin-card ${cardToneClass(plugin)}`;
    const installedVersion = plugin.installedVersion ?? (plugin.installed ? "Unknown" : "Not installed");
    const managedBadge = plugin.managedInstall ? "Managed install" : "Detected install";
    const primaryLabel = actionLabel(plugin);
    const helperText = actionHelperText(primaryLabel);
    card.innerHTML = `
      <header>
        <div class="plugin-heading">
          ${pluginIconMarkup(plugin)}
          <div>
          <h3>${plugin.displayName}</h3>
          </div>
        </div>
        <span class="status-pill ${statusClass(plugin.status)} ${plugin.status === "Ready to install" ? "ready" : ""}">${plugin.status}</span>
      </header>

      <dl class="plugin-meta">
        <div>
          <dt>Installed</dt>
          <dd>${installedVersion}</dd>
        </div>
        <div>
          <dt>Latest</dt>
          <dd>${plugin.latestVersion}</dd>
        </div>
        <div>
          <dt>Location</dt>
          <dd>${plugin.installPath}</dd>
        </div>
        <div>
          <dt>Tracking</dt>
          <dd>${plugin.installed ? managedBadge : "Ready to install"}</dd>
        </div>
      </dl>

      <div class="plugin-actions">
        <button class="${primaryActionClass(primaryLabel)}" data-plugin-id="${plugin.pluginId}" data-action="apply">${primaryLabel}</button>
        ${helperText ? `<p class="action-helper">${helperText}</p>` : ""}
      </div>
      ${pluginOperationMarkup(plugin)}
    `;

    const button = card.querySelector("button");
    button.addEventListener("click", async () => {
      await applyPluginAction(plugin.pluginId, primaryLabel.toLowerCase());
    });

    if (plugin.availableVersions?.length) {
      card.appendChild(renderVersionDrawer(plugin));
    }

    const maintenanceDrawer = renderMaintenanceDrawer(plugin);
    if (maintenanceDrawer) {
      card.appendChild(maintenanceDrawer);
    }

    elements.pluginList.appendChild(card);
  }
}

function renderDashboard() {
  const manager = state.dashboard.manager;
  elements.version.textContent = manager.appVersion;
  elements.platform.textContent = `${manager.platform} / ${manager.arch}`;
  elements.catalogSource.textContent = `${state.dashboard.catalogSource} feed`;
  elements.updaterStatus.textContent = manager.updaterConfigured ? "Configured" : "Not configured";
  renderPlugins();
}

async function refreshDashboard() {
  startOperation("catalog", null, "Refreshing plugin catalog");
  setBusy(true);
  try {
    hideAlert();
    state.dashboard = await invoke("dashboard_state");
    renderDashboard();
    logActivity("Plugin catalog refreshed.");
  } catch (error) {
    const parsed = parseUiError(error, "Couldn't refresh the plugin catalog right now.");
    showAlert(parsed);
    logActivity(`Catalog refresh failed: ${parsed.summary}`);
    elements.pluginList.innerHTML = `<div class="empty-state">${parsed.summary}</div>`;
  } finally {
    finishOperation();
    setBusy(false);
  }
}

async function applyPluginAction(pluginId, action, targetVersion = null) {
  const activeLabel =
    action === "install-selected"
      ? "Installing selected version"
      : action === "uninstall"
        ? "Uninstalling plugin"
        : action === "force-uninstall"
          ? "Force uninstalling plugin"
          : `${action.replace("-", " ").replace(/\b\w/g, (letter) => letter.toUpperCase())} in progress`;
  startOperation(action.includes("uninstall") ? "plugin-uninstall" : "plugin", pluginId, activeLabel);
  setBusy(true);
  try {
    hideAlert();
    const result = await invoke("apply_plugin_action", { pluginId, action, targetVersion });
    logActivity(`${result.pluginId}: ${result.message}`);
    await refreshDashboard();
  } catch (error) {
    const parsed = parseUiError(error, `Couldn't complete the ${action.replace("-", " ")} action for ${pluginId}.`);
    showAlert(parsed);
    logActivity(`${pluginId}: ${parsed.summary}`);
  }
  finally {
    finishOperation();
    setBusy(false);
  }
}

async function checkForManagerUpdates() {
  if (!state.dashboard?.manager?.updaterConfigured) {
    logActivity("Manager updater is not configured in this build yet.");
    return;
  }

  startOperation("manager-update", null, "Updating manager");
  setBusy(true);
  try {
    const update = await check(
      state.dashboard?.manager?.platform === "macos"
        ? { target: "darwin-universal" }
        : undefined
    );
    if (!update) {
      logActivity("Manager app is already up to date.");
      return;
    }

    logActivity(`Downloading manager update ${update.version}.`);
    await update.downloadAndInstall();
    logActivity("Manager update installed. Restarting...");
    await relaunch();
  } catch (error) {
    const parsed = parseUiError(error, "Manager update failed.");
    showAlert(parsed);
    logActivity(`Manager update failed: ${parsed.summary}`);
  } finally {
    finishOperation();
    setBusy(false);
  }
}

elements.refreshButton.addEventListener("click", refreshDashboard);
elements.updateButton.addEventListener("click", checkForManagerUpdates);
elements.alertDismiss.addEventListener("click", hideAlert);

refreshDashboard();
