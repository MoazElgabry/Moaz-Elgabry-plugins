import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { relaunch } from "@tauri-apps/plugin-process";
import { check } from "@tauri-apps/plugin-updater";
import { classifyInvitationVerificationResponse, invitationRequestAction } from "./invitation-flow.js";
import "./styles.css";

const INVITATION_METADATA_KEY = "moaz.pluginManager.invitationMetadata";
const INVITATION_API_BASE = "https://moazelgabry.com/wp-json/moaz-releases/v1";

const state = {
  busy: false,
  dashboard: null,
  activeOperation: null,
  invitation: loadInvitationMetadata(),
  invitationStatusRequest: null,
  access: { deviceId: "", products: [] }
};
const activatedDevelopmentProducts = new Set();

const elements = {
  version: document.querySelector("#manager-version"),
  platform: document.querySelector("#manager-platform"),
  platformIcon: document.querySelector("#manager-platform-icon"),
  managerIdentity: document.querySelector("#manager-identity"),
  catalogSource: document.querySelector("#catalog-source"),
  updaterStatus: document.querySelector("#updater-status"),
  betaToggle: document.querySelector("#beta-releases-toggle"),
  developmentToggle: document.querySelector("#development-builds-toggle"),
  invitationAccessState: document.querySelector("#invitation-access-state"),
  invitationAccessTitle: document.querySelector("#invitation-access-title"),
  invitationAccessEmail: document.querySelector("#invitation-access-email"),
  invitationAccessCopy: document.querySelector("#invitation-access-copy"),
  invitationAccessActions: document.querySelector("#invitation-access-actions"),
  invitationPendingActions: document.querySelector("#invitation-pending-actions"),
  connectApprovedInvitationButton: document.querySelector("#connect-approved-invitation-button"),
  invitationSettingsButton: document.querySelector("#invitation-settings-button"),
  haveInvitationButton: document.querySelector("#have-invitation-button"),
  getInvitationButton: document.querySelector("#get-invitation-button"),
  checkInvitationStatusButton: document.querySelector("#check-invitation-status-button"),
  continueInvitationVerificationButton: document.querySelector("#continue-invitation-verification-button"),
  withdrawInvitationButton: document.querySelector("#withdraw-invitation-button"),
  developmentToken: document.querySelector("#development-invitation-token"),
  developmentConnect: document.querySelector("#connect-development-invitation"),
  developmentForget: document.querySelector("#forget-development-invitation"),
  developmentStatus: document.querySelector("#development-access-status"),
  replaceInvitationButton: document.querySelector("#replace-development-invitation"),
  invitationConnectDialog: document.querySelector("#invitation-connect-dialog"),
  invitationConnectForm: document.querySelector("#invitation-connect-form"),
  invitationConnectClose: document.querySelector("#invitation-connect-close"),
  invitationConnectCancel: document.querySelector("#invitation-connect-cancel"),
  invitationConnectFeedback: document.querySelector("#invitation-connect-feedback"),
  invitationConnectErrorDetails: document.querySelector("#invitation-connect-error-details"),
  invitationConnectErrorMessage: document.querySelector("#invitation-connect-error-message"),
  invitationRequestDialog: document.querySelector("#invitation-request-dialog"),
  invitationRequestForm: document.querySelector("#invitation-request-form"),
  invitationRequestClose: document.querySelector("#invitation-request-close"),
  invitationRequestCancel: document.querySelector("#invitation-request-cancel"),
  invitationRequestEmail: document.querySelector("#invitation-request-email"),
  invitationRequestMessage: document.querySelector("#invitation-request-message"),
  invitationRequestAcknowledgement: document.querySelector("#invitation-request-acknowledgement"),
  invitationRequestSubmit: document.querySelector("#invitation-request-submit"),
  invitationRequestFeedback: document.querySelector("#invitation-request-feedback"),
  invitationVerifyDialog: document.querySelector("#invitation-verify-dialog"),
  invitationVerifyForm: document.querySelector("#invitation-verify-form"),
  invitationVerifyClose: document.querySelector("#invitation-verify-close"),
  invitationVerifyCancel: document.querySelector("#invitation-verify-cancel"),
  invitationVerifyEmail: document.querySelector("#invitation-verify-email"),
  invitationVerificationCode: document.querySelector("#invitation-verification-code"),
  invitationVerifySubmit: document.querySelector("#invitation-verify-submit"),
  invitationVerifyFeedback: document.querySelector("#invitation-verify-feedback"),
  invitationSettingsDialog: document.querySelector("#invitation-settings-dialog"),
  invitationSettingsClose: document.querySelector("#invitation-settings-close"),
  invitationSettingsCopy: document.querySelector("#invitation-settings-copy"),
  licenseDialog: document.querySelector("#license-key-dialog"),
  licenseForm: document.querySelector("#license-key-form"),
  licenseKey: document.querySelector("#license-key"),
  licenseProductName: document.querySelector("#license-product-name"),
  licenseKeyFeedback: document.querySelector("#license-key-feedback"),
  licenseDialogClose: document.querySelector("#license-key-close"),
  licenseDialogCancel: document.querySelector("#license-key-cancel"),
  accessDialog: document.querySelector("#manage-access-dialog"),
  accessDialogTitle: document.querySelector("#manage-access-title"),
  accessDialogBody: document.querySelector("#manage-access-body"),
  accessDialogClose: document.querySelector("#manage-access-close"),
  refreshButton: document.querySelector("#refresh-button"),
  updateButton: document.querySelector("#check-updates-button"),
  supportButton: document.querySelector("#support-button"),
  pluginList: document.querySelector("#plugin-list"),
  activityLog: document.querySelector("#activity-log"),
  alertBanner: document.querySelector("#alert-banner"),
  alertLabel: document.querySelector("#alert-banner .alert-label"),
  alertSummary: document.querySelector("#alert-summary"),
  alertMessage: document.querySelector("#alert-message"),
  alertDetails: document.querySelector("#alert-details"),
  alertDismiss: document.querySelector("#alert-dismiss"),
  releaseHighlightsDialog: document.querySelector("#release-highlights-dialog"),
  releaseHighlightsTitle: document.querySelector("#release-highlights-title"),
  releaseHighlightsBody: document.querySelector("#release-highlights-body"),
  releaseHighlightsLink: document.querySelector("#release-highlights-link"),
  releaseHighlightsClose: document.querySelector("#release-highlights-close"),
  diagnosticsExportDialog: document.querySelector("#diagnostics-export-dialog"),
  diagnosticsExportTitle: document.querySelector("#diagnostics-export-title"),
  diagnosticsExportCopy: document.querySelector("#diagnostics-export-copy"),
  diagnosticsClearLogs: document.querySelector("#diagnostics-clear-logs"),
  diagnosticsExportStart: document.querySelector("#diagnostics-export-start"),
  diagnosticsExportCancel: document.querySelector("#diagnostics-export-cancel")
};

const alertBannerHome = {
  parent: elements.alertBanner?.parentNode ?? null,
  nextSibling: elements.alertBanner?.nextSibling ?? null
};

const invitationDialogs = [
  elements.invitationConnectDialog,
  elements.invitationRequestDialog,
  elements.invitationVerifyDialog,
  elements.invitationSettingsDialog,
  elements.licenseDialog,
  elements.accessDialog
].filter(Boolean);

/**
 * WordPress invitation request contract (v1):
 *
 * POST /invitation-requests
 *   { email: string, plugin_slugs: string[], message: string,
 *     verification_code?: string }
 * POST /invitation-requests/verify
 *   { request_id: string|number, code: string }
 * GET /invitation-requests/status?receipt=string
 * POST /invitation-requests/withdraw
 *   { receipt: string }
 *
 * Request responses may include request_id, receipt, email, status,
 * verification_required, already_approved, plugin_slugs, and invitation_key/token.
 * The key is passed directly to the native validate-before-store command and is
 * never persisted here.
 * Cooldown errors include code moaz_er_invitation_cooldown and a UTC
 * cooldown_until value in the REST error data.
 */
function loadInvitationMetadata() {
  try {
    const raw = localStorage.getItem(INVITATION_METADATA_KEY);
    const parsed = raw ? JSON.parse(raw) : {};
    return {
      status: ["pending", "verification_required", "approved_key_confirmation"].includes(parsed?.status) ? parsed.status : "disconnected",
      email: typeof parsed?.email === "string" ? parsed.email : "",
      // The receipt is a bearer credential and is hydrated from the OS
      // credential store below. Older builds may have left one in localStorage;
      // hydrateInvitationReceipt migrates it once and removes it.
      receipt: "",
      requestId: parsed?.requestId == null ? "" : String(parsed.requestId)
    };
  } catch {
    return { status: "disconnected", email: "", receipt: "", requestId: "" };
  }
}

function persistInvitationMetadata() {
  try {
    localStorage.setItem(INVITATION_METADATA_KEY, JSON.stringify({
      status: ["pending", "verification_required", "approved_key_confirmation"].includes(state.invitation.status) ? state.invitation.status : "disconnected",
      email: state.invitation.email,
      requestId: state.invitation.requestId
    }));
  } catch {
    // Metadata is a convenience only; the secure invitation remains native.
  }
}

function clearInvitationMetadata() {
  state.invitation = { status: "disconnected", email: "", receipt: "", requestId: "" };
  activatedDevelopmentProducts.clear();
  invoke("forget_invitation_request_receipt").catch(() => {});
  try {
    localStorage.removeItem(INVITATION_METADATA_KEY);
  } catch {
    // Ignore storage failures; no secret is kept in localStorage.
  }
}

async function hydrateInvitationReceipt() {
  try {
    const receipt = await invoke("invitation_request_receipt");
    if (typeof receipt === "string" && receipt) {
      state.invitation.receipt = receipt;
    }
    // Migrate a receipt written by an older manager build into the OS store.
    const legacy = (() => {
      try {
        const raw = localStorage.getItem(INVITATION_METADATA_KEY);
        const parsed = raw ? JSON.parse(raw) : null;
        return typeof parsed?.receipt === "string" ? parsed.receipt : "";
      } catch {
        return "";
      }
    })();
    if (!state.invitation.receipt && legacy) {
      await invoke("store_invitation_request_receipt", { receipt: legacy });
      state.invitation.receipt = legacy;
    }
    try {
      const raw = localStorage.getItem(INVITATION_METADATA_KEY);
      if (raw) {
        const parsed = JSON.parse(raw);
        if (parsed && Object.prototype.hasOwnProperty.call(parsed, "receipt")) {
          delete parsed.receipt;
          localStorage.setItem(INVITATION_METADATA_KEY, JSON.stringify(parsed));
        }
      }
    } catch {
      // The request can still be checked during this session.
    }
  } catch {
    // Credential-store unavailability must not disable public catalog access.
  }
}

function invitationResponseValue(payload, ...keys) {
  for (const key of keys) {
    if (payload && payload[key] != null && payload[key] !== "") return payload[key];
  }
  return null;
}

function normalizeInvitationResponse(payload) {
  const body = payload?.data && typeof payload.data === "object" ? payload.data : payload;
  return {
    requestId: invitationResponseValue(body, "request_id", "requestId", "id"),
    receipt: invitationResponseValue(body, "receipt", "status_receipt"),
    email: invitationResponseValue(body, "email") ?? "",
    status: String(invitationResponseValue(body, "status") ?? "pending").toLowerCase(),
    verificationRequired: Boolean(invitationResponseValue(body, "verification_required", "verificationRequired")),
    alreadyApproved: Boolean(invitationResponseValue(body, "already_approved", "alreadyApproved")),
    token: invitationResponseValue(body, "invitation_key", "invitationKey", "license_key", "licenseKey", "token", "key")
  };
}

function invitationApiError(status, message, code = "", data = {}) {
  const error = new Error(message || `Invitation service returned HTTP ${status}.`);
  error.status = status;
  error.code = code;
  error.cooldownUntil = typeof data?.cooldown_until === "string" ? data.cooldown_until : "";
  error.automaticCooldown = Boolean(data?.automatic);
  return error;
}

const invitationApi = {
  async request({ email, pluginSlugs, message, verificationCode, purpose }) {
    return invitationApi.fetch("/invitation-requests", {
      method: "POST",
      body: {
        email,
        plugin_slugs: pluginSlugs,
        message,
        ...(purpose ? { purpose } : {}),
        ...(verificationCode ? { verification_code: verificationCode } : {})
      }
    });
  },
  async verify({ requestId, code }) {
    return invitationApi.fetch("/invitation-requests/verify", {
      method: "POST",
      body: { request_id: requestId, code }
    });
  },
  async status(receipt) {
    return invitationApi.fetch(`/invitation-requests/status?receipt=${encodeURIComponent(receipt)}`, { method: "GET" });
  },
  async withdraw(receipt) {
    return invitationApi.fetch("/invitation-requests/withdraw", {
      method: "POST",
      body: { receipt }
    });
  },
  async fetch(path, { method, body }) {
    let response;
    try {
      response = await window.fetch(`${INVITATION_API_BASE}${path}`, {
        method,
        headers: { Accept: "application/json", ...(body ? { "Content-Type": "application/json" } : {}) },
        body: body ? JSON.stringify(body) : undefined
      });
    } catch (error) {
      throw invitationApiError(0, "The invitation service could not be reached. Check your connection and try again.");
    }
    let payload = null;
    try {
      payload = await response.json();
    } catch {
      // Some gateways return an empty body for 404/503; preserve the status.
    }
    if (!response.ok) {
      const message = payload?.message || payload?.error || (response.status === 404
        ? "Invitation requests are not available yet. Please contact support@moazelgabry.com."
        : "The invitation service could not complete that request.");
      throw invitationApiError(response.status, message, payload?.code || "", payload?.data || {});
    }
    return normalizeInvitationResponse(payload ?? {});
  }
};

function logActivity(message) {
  const item = document.createElement("div");
  item.className = "activity-item";
  item.innerHTML = `<time>${new Date().toLocaleString()}</time><div>${message}</div>`;
  elements.activityLog.prepend(item);
}

function bindEvent(element, eventName, handler) {
  element?.addEventListener(eventName, handler);
}

function setBusy(nextBusy) {
  state.busy = nextBusy;
  document.querySelectorAll("button, select, input").forEach((element) => {
    element.disabled = nextBusy;
  });
  if (!nextBusy) {
    applyReleaseControlState();
  }
}

function applyReleaseControlState() {
  const manager = state.dashboard?.manager;
  if (!manager) return;
  const developmentEnabled = Boolean(manager.developmentBuildsEnabled);
  const invitationConnected = Boolean(manager.developmentInvitationConnected);
  const invitationHasNoPluginAccess = invitationConnected && manager.developmentInvitationHasAccess === false;
  const invitationRequestActive = ["pending", "verification_required", "approved_key_confirmation"].includes(state.invitation.status);
  const invitationRequestTrackable = (state.invitation.status === "pending" && Boolean(state.invitation.receipt)) || state.invitation.status === "approved_key_confirmation";
  elements.betaToggle.checked = Boolean(manager.betaReleasesEnabled);
  elements.betaToggle.disabled = state.busy;
  elements.betaToggle.closest(".toggle-row")?.classList.remove("forced-toggle");
  elements.developmentToggle.checked = developmentEnabled;
  elements.developmentToggle.disabled = state.busy;
  if (elements.invitationAccessState) {
    elements.invitationAccessState.dataset.status = invitationHasNoPluginAccess
      ? "no_access"
      : invitationConnected ? "connected" : state.invitation.status;
    elements.invitationAccessState.classList.toggle(
      "hidden",
      !invitationConnected && !invitationRequestActive
    );
  }
  if (elements.invitationAccessTitle) {
    const title = invitationConnected
      ? "Connected as"
      : state.invitation.status === "verification_required"
        ? "Email verification required"
        : state.invitation.status === "approved_key_confirmation"
          ? "Approved : key confirmation pending"
          : state.invitation.status === "pending" ? "Invitation request pending" : "No invitation connected";
    if (state.invitation.status === "approved_key_confirmation" && !invitationConnected) {
      const detail = document.createElement("span");
      detail.className = "invitation-access-title-detail";
      detail.textContent = "key confirmation pending";
      elements.invitationAccessTitle.replaceChildren(document.createTextNode("Approved : "), detail);
    } else {
      elements.invitationAccessTitle.textContent = title;
    }
  }
  if (elements.invitationAccessEmail) {
    elements.invitationAccessEmail.textContent = invitationConnected || invitationRequestActive ? state.invitation.email : "";
  }
  if (elements.invitationAccessCopy) {
    const developmentWarning = state.dashboard?.developmentWarning?.trim();
    elements.invitationAccessCopy.classList.toggle(
      "hidden",
      invitationConnected && !invitationHasNoPluginAccess && !developmentWarning
    );
    elements.invitationAccessCopy.classList.toggle("invitation-access-copy--revoked", invitationHasNoPluginAccess);
    elements.invitationAccessCopy.textContent = invitationConnected
      ? invitationHasNoPluginAccess
        ? "Access to all plugins has been revoked. Contact support@moazelgabry.com if you need help."
        : developmentWarning ?? ""
      : state.invitation.status === "verification_required"
        ? "Enter the verification code we emailed to finish your request."
        : state.invitation.status === "approved_key_confirmation"
          ? "Approved. Check your email for your code; contact support@moazelgabry.com if it’s missing."
        : state.invitation.status === "pending"
          ? state.invitation.receipt
            ? "We’ll email you with updates on your invitation request."
            : "The secure receipt is missing, so this request can’t be withdrawn here. Forgetting it only removes the local record; contact support to cancel it online."
          : "Connect an invitation to use private development builds.";
  }
  elements.invitationAccessActions?.classList.toggle("hidden", invitationConnected || invitationRequestActive);
  elements.invitationPendingActions?.classList.toggle("hidden", invitationConnected || !invitationRequestTrackable);
  elements.checkInvitationStatusButton?.classList.toggle("hidden", !state.invitation.receipt);
  elements.withdrawInvitationButton?.classList.toggle("hidden", state.invitation.status === "approved_key_confirmation");
  elements.connectApprovedInvitationButton?.classList.toggle("hidden", state.invitation.status !== "approved_key_confirmation");
  elements.continueInvitationVerificationButton?.classList.toggle(
    "hidden",
    invitationConnected || state.invitation.status !== "verification_required"
  );
  elements.invitationSettingsButton?.classList.toggle("hidden", !invitationConnected && !invitationRequestActive);
  elements.replaceInvitationButton?.classList.toggle("hidden", !invitationConnected);
  if (elements.invitationSettingsCopy) {
    elements.invitationSettingsCopy.textContent = invitationConnected
      ? invitationHasNoPluginAccess
        ? "Access to all plugins has been revoked. Contact support@moazelgabry.com if you need help."
        : "Manage your invitation access."
      : state.invitation.status === "verification_required"
        ? "Enter the verification code sent to your email to finish this request."
        : state.invitation.status === "approved_key_confirmation"
          ? "Your access is approved. Check your email for the invitation code, or contact support@moazelgabry.com if it’s missing."
        : state.invitation.receipt
          ? "Your invitation request is pending. You can check its status or withdraw it here."
          : "The secure receipt is missing, so this request can’t be withdrawn here. Forgetting it only removes the local record; contact support to cancel it online.";
  }
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
  if (kind === "plugin-logs") {
    return [
      "Launching Resolve for diagnostics",
      "Reproduce the issue, then close Resolve",
      "Collecting logs after Resolve closes"
    ];
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

  if (kind !== "plugin-logs") {
    state.activeOperation.timer = window.setInterval(() => {
      if (!state.activeOperation || state.activeOperation.kind !== kind || state.activeOperation.pluginId !== pluginId) {
        return;
      }
      const lastStep = state.activeOperation.steps.length - 1;
      state.activeOperation.stepIndex = Math.min(state.activeOperation.stepIndex + 1, lastStep);
      renderPlugins();
    }, 1400);
  }

  renderPlugins();
}

function finishOperation() {
  if (state.activeOperation?.timer) {
    window.clearInterval(state.activeOperation.timer);
  }
  state.activeOperation = null;
  renderPlugins();
}

function updateOperationProgress({ label, steps, stepIndex = 0 } = {}) {
  if (!state.activeOperation) {
    return;
  }

  if (label) {
    state.activeOperation.label = label;
  }
  if (steps) {
    state.activeOperation.steps = steps;
  }
  state.activeOperation.stepIndex = Math.max(0, Math.min(stepIndex, state.activeOperation.steps.length - 1));
  renderPlugins();
}

function parseUiError(error, fallbackSummary = "The operation failed.") {
  const raw = typeof error === "string" ? error : String(error);

  if (
    raw.includes("fallback platforms") &&
    raw.includes("response `platforms` object")
  ) {
    return {
      summary: "Update is still being published. Try again in a minute.",
      details:
        "The new manager release is available, but the update feed has not finished refreshing yet. Wait a moment and check again.",
      code: "updater_feed_pending"
    };
  }

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
  elements.alertLabel.textContent = payload.type === "info" ? "Information" : "Attention";
  const hasDetails = Boolean(payload.details) && payload.details !== payload.summary;
  elements.alertMessage.textContent = payload.details ?? "";
  elements.alertDetails.classList.toggle("hidden", !hasDetails);
  elements.alertDetails.open = false;
  elements.alertBanner.classList.toggle("is-info", payload.type === "info");
  elements.alertBanner.classList.remove("hidden");
  syncAlertLayer();
}

function hideAlert() {
  elements.alertBanner.classList.add("hidden");
  elements.alertBanner.classList.remove("is-info");
  restoreAlertBanner();
  elements.alertSummary.textContent = "";
  elements.alertMessage.textContent = "";
  elements.alertDetails.classList.add("hidden");
  elements.alertDetails.open = false;
}

function openInvitationDialog() {
  return invitationDialogs.find((dialog) => dialog.open) ?? null;
}

function moveAlertIntoDialog(dialog) {
  if (!dialog || elements.alertBanner.parentNode === dialog) return;
  dialog.appendChild(elements.alertBanner);
  elements.alertBanner.classList.add("alert-banner-in-dialog");
}

function restoreAlertBanner() {
  if (!alertBannerHome.parent || elements.alertBanner.parentNode === alertBannerHome.parent) return;
  const nextSibling = alertBannerHome.nextSibling?.parentNode === alertBannerHome.parent
    ? alertBannerHome.nextSibling
    : null;
  alertBannerHome.parent.insertBefore(elements.alertBanner, nextSibling);
  elements.alertBanner.classList.remove("alert-banner-in-dialog");
}

function syncAlertLayer() {
  if (elements.alertBanner.classList.contains("hidden")) {
    restoreAlertBanner();
    return;
  }
  const dialog = openInvitationDialog();
  if (dialog) {
    moveAlertIntoDialog(dialog);
  } else {
    restoreAlertBanner();
  }
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function hasReleaseHighlights(value) {
  if (typeof value !== "string") return false;
  const withoutComments = value
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<[^>]*>/g, " ")
    .replace(/&nbsp;|&#160;|&#xA0;/gi, " ")
    .replace(/[\u200B-\u200D\uFEFF]/g, "");
  return withoutComments
    .split(/\r?\n/)
    .some((line) => line.trim().replace(/^[-*+]\s*/, "").trim().length > 0);
}

function hasVersionHighlights(plugin, version) {
  // The main card owns the latest release highlights. Avoid a duplicate info
  // button in version history for the same build, whose version entry may not
  // carry the manifest-level highlights field.
  if (!version || version.version === plugin.latestVersion) return false;
  return hasReleaseHighlights(version.releaseHighlights);
}

function renderReleaseHighlightsMarkup(raw) {
  if (!hasReleaseHighlights(raw)) {
    return "<p>No version highlights were provided for this release.</p>";
  }

  const blocks = [];
  let bulletItems = [];

  const flushBullets = () => {
    if (!bulletItems.length) {
      return;
    }
    blocks.push(`<ul>${bulletItems.map((item) => `<li>${escapeHtml(item)}</li>`).join("")}</ul>`);
    bulletItems = [];
  };

  for (const line of raw.replaceAll("\r\n", "\n").split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) {
      flushBullets();
      continue;
    }

    if (/^[-*+]\s/.test(trimmed)) {
      const item = trimmed.slice(1).trim();
      if (item) bulletItems.push(item);
      continue;
    }

    flushBullets();
    blocks.push(`<p>${escapeHtml(trimmed)}</p>`);
  }

  flushBullets();
  return blocks.join("");
}

function openReleaseHighlightsDialog({ pluginName, version, releaseNotesUrl, releaseHighlights }) {
  elements.releaseHighlightsTitle.textContent = `${pluginName} ${version}`;
  elements.releaseHighlightsBody.innerHTML = renderReleaseHighlightsMarkup(releaseHighlights);

  if (releaseNotesUrl) {
    elements.releaseHighlightsLink.href = releaseNotesUrl;
    elements.releaseHighlightsLink.hidden = false;
  } else {
    elements.releaseHighlightsLink.hidden = true;
    elements.releaseHighlightsLink.removeAttribute("href");
  }

  if (elements.releaseHighlightsDialog.open) {
    elements.releaseHighlightsDialog.close();
  }
  elements.releaseHighlightsDialog.showModal();
}

function closeReleaseHighlightsDialog() {
  if (elements.releaseHighlightsDialog.open) {
    elements.releaseHighlightsDialog.close();
  }
}

function showDiagnosticsExportDialog(displayName) {
  elements.diagnosticsExportTitle.textContent = displayName;
  elements.diagnosticsExportCopy.textContent =
    "This will start DaVinci Resolve in diagnostics mode. After Resolve opens, reproduce the issue you want to report, then close Resolve. The manager will collect the generated logs after Resolve closes.";
  elements.diagnosticsClearLogs.checked = false;

  return new Promise((resolve) => {
    let settled = false;
    const cleanup = () => {
      elements.diagnosticsExportStart.removeEventListener("click", handleStart);
      elements.diagnosticsExportCancel.removeEventListener("click", handleCancel);
      elements.diagnosticsExportDialog.removeEventListener("cancel", handleCancel);
      elements.diagnosticsExportDialog.removeEventListener("click", handleBackdropClick);
      elements.diagnosticsExportDialog.removeEventListener("close", handleClose);
    };
    const settle = (value) => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(value);
    };
    const closeDialog = () => {
      if (elements.diagnosticsExportDialog.open) {
        elements.diagnosticsExportDialog.close();
      }
    };
    const handleStart = () => {
      const removePreviousLogs = elements.diagnosticsClearLogs.checked;
      settle({ removePreviousLogs });
      closeDialog();
    };
    const handleCancel = (event) => {
      event?.preventDefault();
      settle(null);
      closeDialog();
    };
    const handleBackdropClick = (event) => {
      if (event.target === elements.diagnosticsExportDialog) {
        handleCancel(event);
      }
    };
    const handleClose = () => {
      settle(null);
    };

    elements.diagnosticsExportStart.addEventListener("click", handleStart);
    elements.diagnosticsExportCancel.addEventListener("click", handleCancel);
    elements.diagnosticsExportDialog.addEventListener("cancel", handleCancel);
    elements.diagnosticsExportDialog.addEventListener("click", handleBackdropClick);
    elements.diagnosticsExportDialog.addEventListener("close", handleClose);
    elements.diagnosticsExportDialog.showModal();
  });
}

function statusClass(status) {
  if (status === "Installed" || status === "Up to date") return "ok";
  if (
    status === "Update available" ||
    status === "Stable available" ||
    status === "Stable update available" ||
    status === "Beta installed" ||
    status === "Catalog behind" ||
    status === "Unmanaged install"
  ) {
    return "warn";
  }
  return "bad";
}

function actionLabel(plugin) {
  if (!plugin.installed) return "Install";
  if (plugin.channelSwitchMode === "stable_update_available") return "Update to stable";
  if (plugin.channelSwitchMode === "return_to_stable") return "Install stable";
  if (plugin.catalogBehindInstalled) return "Reinstall";
  if (plugin.needsUpdate) return "Update";
  return "Reinstall";
}

function actionRequest(plugin) {
  if (!plugin.installed) return "install";
  if (plugin.channelSwitchAvailable) return "update";
  if (plugin.needsUpdate) return "update";
  return "reinstall";
}

function rollbackButtonLabel(plugin, selectedVersion) {
  if (!selectedVersion) return "Install selected";
  const selected = plugin.availableVersions?.find((option) => option.version === selectedVersion);
  const label = selected?.actionLabel ?? "Install selected";
  return label === "Reinstall this development build" ? "Reinstall Dev build" : label;
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

  if (plugin.channelSwitchMode === "stable_update_available") {
    if (selected.isCurrentLatest) {
      return `This installs the newly released stable version (${selected.version}) over the current beta build (${plugin.installedVersion}).`;
    }

    return `This installs stable version ${selected.version} instead of the current beta build (${plugin.installedVersion}).`;
  }

  if (plugin.channelSwitchMode === "return_to_stable") {
    if (selected.isCurrentLatest) {
      return `This installs the latest stable release (${selected.version}) and moves ${plugin.displayName} off the current beta build (${plugin.installedVersion}).`;
    }

    return `This installs ${selected.version} instead of the current beta build (${plugin.installedVersion}).`;
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

function findVersionOption(plugin, version) {
  return plugin.availableVersions?.find((option) => option.version === version) ?? null;
}

function releaseInfoButtonMarkup(className = "") {
  const resolvedClassName = className ? `release-info-button ${className}` : "release-info-button";
  return `
    <button
      type="button"
      class="${resolvedClassName}"
      aria-label="View version highlights"
      title="View version highlights"
    >
      <span aria-hidden="true">i</span>
    </button>
  `;
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

function actionHelperText(plugin, primaryLabel) {
  if (plugin.catalogBehindInstalled) {
    if (state.dashboard?.catalogSource === "local-dev") {
      return `The local dev catalog currently lists ${plugin.latestVersion}, but the detected installed version (${plugin.installedVersion}) is newer. Update the local dev manifest or switch back to the remote feed if this looks wrong.`;
    }
    return `The catalog currently lists ${plugin.latestVersion}, but the detected installed version (${plugin.installedVersion}) is newer. Refresh the catalog if this looks wrong.`;
  }
  if (primaryLabel === "Update to stable") return "Install the newly released stable version.";
  if (primaryLabel === "Install stable") return "Leave beta and install the latest stable release.";
  if (primaryLabel === "Update") return "Install the latest release.";
  if (primaryLabel === "Reinstall") return "Reinstall the current version.";
  return "";
}

function pluginOperationMarkup(plugin) {
  const operation = state.activeOperation;
  if (!operation || operation.pluginId !== plugin.pluginId) return "";

  const step = operation.steps[operation.stepIndex] ?? operation.label;
  const showStep = step && step !== operation.label;
  return `
    <div class="plugin-progress" role="status" aria-live="polite">
      <div class="plugin-progress-copy">
        <p class="plugin-progress-label">${operation.label}</p>
        ${showStep ? `<p class="plugin-progress-step">${step}</p>` : ""}
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
  const iconUrl = normalizeIconUrl(plugin.iconUrl);
  const initial = escapeHtml(plugin.displayName.charAt(0));
  if (iconUrl) {
    return `
      <div class="plugin-icon plugin-icon-has-image" aria-hidden="true">
        <img src="${escapeHtml(iconUrl)}" alt="" loading="lazy" />
        <span>${initial}</span>
      </div>
    `;
  }

  return `
    <div class="plugin-icon plugin-icon-fallback" aria-hidden="true">
      <span>${initial}</span>
    </div>
  `;
}

function diagnosticsButtonMarkup(className = "") {
  const resolvedClassName = className ? `diagnostics-button ${className}` : "diagnostics-button";
  return `
    <button
      type="button"
      class="${resolvedClassName}"
      aria-label="Export diagnostics logs"
      title="Export diagnostics logs"
    >
      <span class="diagnostics-icon" aria-hidden="true"></span>
    </button>
  `;
}

function diagnosticsAvailable(plugin) {
  return Boolean(plugin.installed && plugin.diagnostics?.enabled);
}

function pluginNameMarkup(plugin) {
  const developmentBuild = plugin.releaseChannel === "dev";
  const developmentReceiptStatus = plugin.developmentReceiptStatus ?? "inactive";
  const access = plugin.accessMode === "licensed" && !developmentBuild
    ? accessForProduct(plugin.pluginId)
    : null;
  const accessStatus = access?.status === "not_activated" && access.licenses?.some((license) => license.keyAvailable)
    ? "activation_required"
    : (access?.status ?? "not_activated");
  return `
    <span>${escapeHtml(plugin.displayName)}</span>
    ${plugin.accessMode === "free" ? '<span class="plugin-channel-tag plugin-free-tag">Free</span>' : ""}
    ${plugin.accessMode === "licensed" && developmentBuild ? `<span class="plugin-channel-tag plugin-invitation-access-tag" data-receipt-status="${escapeHtml(developmentReceiptStatus)}" title="Development receipt: ${escapeHtml(developmentReceiptStatus)}">Invitation access</span>` : ""}
    ${plugin.accessMode === "licensed" && !developmentBuild ? `<span class="plugin-channel-tag plugin-access-tag" data-access-status="${escapeHtml(accessStatus)}">${escapeHtml(accessStatusLabel(accessStatus))}</span>` : ""}
    ${plugin.accessMode === "unknown" ? '<span class="plugin-channel-tag plugin-access-unknown-tag">Access setup needed</span>' : ""}
    ${plugin.releaseChannel === "dev" ? '<span class="plugin-channel-tag plugin-dev-tag">Development</span>' : ""}
    ${plugin.betaRelease ? '<span class="plugin-channel-tag plugin-beta-tag">Beta</span>' : ""}
  `;
}

function normalizeIconUrl(raw) {
  if (typeof raw !== "string") return "";
  const value = raw.trim();
  if (!value) return "";
  if (/^[a-zA-Z]:[\\/]/.test(value)) {
    return `file:///${value.replaceAll("\\", "/")}`;
  }
  if (value.startsWith("/")) {
    return `file://${value}`;
  }
  if (value.startsWith("\\\\")) {
    return `file:${value.replaceAll("\\", "/")}`;
  }
  return value;
}

function renderVersionDrawer(plugin) {
  const initialVersion = plugin.installedVersion ?? plugin.availableVersions[0]?.version ?? "";
  const initialSelected = findVersionOption(plugin, initialVersion);
  const showInitialInfo = hasVersionHighlights(plugin, initialSelected);
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
        <div class="version-picker-actions">
          <button type="button" data-plugin-id="${plugin.pluginId}" data-action="install-selected">Install selected</button>
          ${showInitialInfo ? releaseInfoButtonMarkup("rollback-info-button") : ""}
        </div>
      </div>
      <p class="version-hint"></p>
    </div>
  `;

  const select = wrapper.querySelector("select");
  const installSelectedButton = wrapper.querySelector('[data-action="install-selected"]');
  const hint = wrapper.querySelector(".version-hint");

  const refreshCopy = () => {
    const selected = findVersionOption(plugin, select.value);
    installSelectedButton.textContent = rollbackButtonLabel(plugin, select.value);
    hint.textContent = selectedVersionHint(plugin, select.value);
    const actions = wrapper.querySelector(".version-picker-actions");
    let infoButton = actions.querySelector(".rollback-info-button");
    const shouldShowInfo = hasVersionHighlights(plugin, selected);

    if (shouldShowInfo && !infoButton) {
      actions.insertAdjacentHTML("beforeend", releaseInfoButtonMarkup("rollback-info-button"));
      infoButton = actions.querySelector(".rollback-info-button");
      infoButton.addEventListener("click", () => {
        const selectedVersion = findVersionOption(plugin, select.value);
        if (!selectedVersion || !hasReleaseHighlights(selectedVersion.releaseHighlights)) {
          return;
        }
        openReleaseHighlightsDialog({
          pluginName: plugin.displayName,
          version: selectedVersion.version,
          releaseNotesUrl: selectedVersion.releaseNotesUrl,
          releaseHighlights: selectedVersion.releaseHighlights
        });
      });
    } else if (!shouldShowInfo && infoButton) {
      infoButton.remove();
    }
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
    const primaryRequest = actionRequest(plugin);
    const helperText = actionHelperText(plugin, primaryLabel);
    const showLatestInfo = hasReleaseHighlights(plugin.releaseHighlights);
    const showDiagnostics = diagnosticsAvailable(plugin);
    card.innerHTML = `
      <header>
        <div class="plugin-heading">
          ${pluginIconMarkup(plugin)}
          <h3>${pluginNameMarkup(plugin)}</h3>
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
        <button type="button" class="${primaryActionClass(primaryLabel)}" data-plugin-id="${plugin.pluginId}" data-action="${primaryRequest}">${primaryLabel}</button>
        ${
          helperText
            ? `<p class="action-helper">${helperText}</p>`
            : '<span class="action-helper-placeholder" aria-hidden="true"></span>'
        }
        <div class="plugin-action-icons">
          ${showDiagnostics ? diagnosticsButtonMarkup("main-action-diagnostics-button") : ""}
          ${showLatestInfo ? releaseInfoButtonMarkup("main-action-info-button") : ""}
        </div>
      </div>
      ${pluginAccessMarkup(plugin)}
      ${pluginOperationMarkup(plugin)}
    `;

    const button = card.querySelector(`[data-action="${primaryRequest}"]`);
    button.addEventListener("click", async () => {
      await applyPluginAction(plugin.pluginId, primaryRequest);
    });
    const infoButton = card.querySelector(".main-action-info-button");
    if (infoButton) {
      infoButton.addEventListener("click", () => {
        openReleaseHighlightsDialog({
          pluginName: plugin.displayName,
          version: plugin.latestVersion,
          releaseNotesUrl: plugin.releaseNotesUrl,
          releaseHighlights: plugin.releaseHighlights
        });
      });
    }
    const diagnosticsButton = card.querySelector(".main-action-diagnostics-button");
    if (diagnosticsButton) {
      diagnosticsButton.addEventListener("click", async () => {
        await exportPluginLogs(plugin.pluginId);
      });
    }
    card.querySelectorAll("[data-license-action]").forEach((licenseButton) => {
      licenseButton.addEventListener("click", () => {
        const productSlug = licenseButton.dataset.licenseProduct;
        const action = licenseButton.dataset.licenseAction;
        const target = state.dashboard?.plugins?.find((item) => item.pluginId === productSlug);
        if (!target) return;
        if (action === "enter") openLicenseDialog(target);
        if (action === "manage") openManageAccessDialog(target);
        if (action === "get" && target.licenseUrl) window.open(target.licenseUrl, "_blank", "noopener,noreferrer");
      });
    });

    const iconImage = card.querySelector(".plugin-icon img");
    if (iconImage) {
      iconImage.addEventListener(
        "error",
        () => {
          const icon = iconImage.closest(".plugin-icon");
          if (!icon) return;
          icon.classList.remove("plugin-icon-has-image");
          icon.classList.add("plugin-icon-fallback");
          iconImage.remove();
        },
        { once: true }
      );
    }

    if ((plugin.availableVersions?.length ?? 0) > 1) {
      card.appendChild(renderVersionDrawer(plugin));
    }

    const maintenanceDrawer = renderMaintenanceDrawer(plugin);
    if (maintenanceDrawer) {
      card.appendChild(maintenanceDrawer);
    }

    elements.pluginList.appendChild(card);
  }
}

function accessForProduct(productSlug) {
  return state.access?.products?.find((product) => product.productSlug === productSlug) ?? null;
}

function accessStatusLabel(status) {
  return {
    active: "Licensed",
    not_activated: "License required",
    activation_required: "Activation needed",
    expired: "Expired",
    device_limit_reached: "No seats available",
    revoked: "Revoked",
    invalid: "Check license",
    verification_unconfigured: "Licensing unavailable",
    unknown: "Access unavailable"
  }[status] ?? "License required";
}

function accessExpiryLabel(expiresAt) {
  if (!expiresAt) return "";
  return ` · Expires ${new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(new Date(expiresAt * 1000))}`;
}

function pluginAccessMarkup(plugin) {
  if (plugin.accessMode !== "licensed" || plugin.releaseChannel === "dev") return "";
  const access = accessForProduct(plugin.pluginId);
  const status = access?.status ?? "not_activated";
  const activated = access?.licenses?.length > 0;
  return `
    <section class="product-access ${status}" aria-label="${escapeHtml(plugin.displayName)} product access">
      <div class="product-access-summary">
        <span class="license-status-pill" data-status="${status}">${accessStatusLabel(status)}${accessExpiryLabel(access?.expiresAt)}</span>
        <span class="product-access-source">${activated ? `${access.licenses.length} license source${access.licenses.length === 1 ? "" : "s"}` : "No license connected"}</span>
      </div>
      <div class="product-access-actions">
        <button type="button" class="subtle-button" data-license-product="${escapeHtml(plugin.pluginId)}" data-license-action="enter">Enter license key</button>
        ${activated ? `<button type="button" class="subtle-button" data-license-product="${escapeHtml(plugin.pluginId)}" data-license-action="manage">Manage access</button>` : ""}
        ${plugin.licenseUrl ? `<button type="button" class="subtle-button" data-license-product="${escapeHtml(plugin.pluginId)}" data-license-action="get">Get a license</button>` : ""}
      </div>
    </section>
  `;
}

function managerPlatformIconMarkup(platform) {
  const icons = {
    windows: `<svg viewBox="0 0 24 24" focusable="false"><path fill="currentColor" d="M2.5 5.1 10.5 4v7.2h-8V5.1Zm9.2-1.3L21.5 2v9.2h-9.8V3.8ZM2.5 12.8h8v7.2l-8-1.1v-6.1Zm9.2 0h9.8V22l-9.8-1.4v-7.8Z"/></svg>`,
    macos: `<svg viewBox="0 0 24 24" focusable="false"><path fill="currentColor" d="M18.7 19.5c-.9 1.4-2 2.8-3.6 2.8-1.5.1-2-.9-3.8-.9-1.7 0-2.3.9-3.8 1-1.5 0-2.7-1.6-3.7-3-2-2.9-3.5-8.2-1.4-11.8a6.7 6.7 0 0 1 4.9-3c1.5 0 3 1 3.9 1 .9 0 2.7-1.3 4.5-1.1.8 0 2.9.3 4.3 2.3-.1.1-2.6 1.5-2.5 4.5 0 3.5 3.1 4.7 3.1 4.7 0 .1-.5 1.8-1.9 3.5ZM13.4 3.7c.8-1 1.3-2.3 1.2-3.7-1.2.1-2.6.8-3.4 1.7-.8.9-1.4 2.2-1.2 3.5 1.2.1 2.6-.6 3.4-1.5Z"/></svg>`,
    linux: `<svg viewBox="0 0 24 24" focusable="false"><ellipse cx="12" cy="14.2" rx="7.4" ry="8.2" fill="currentColor"/><ellipse cx="12" cy="15.2" rx="4.4" ry="6.3" fill="#e7edf2"/><circle cx="12" cy="7" r="4.8" fill="currentColor"/><circle cx="10.3" cy="6.7" r=".7" fill="#1c242b"/><circle cx="13.7" cy="6.7" r=".7" fill="#1c242b"/><path d="m10.2 8.2 1.8 1.5 1.8-1.5-1.8-.5-1.8.5Z" fill="#d6a34c"/><path d="M5.5 21.1 8 19.8l1.2 1.6-2.7.7-1-.9Zm13 0-2.5-1.3-1.2 1.6 2.7.7 1-.9Z" fill="#d6a34c"/></svg>`
  };
  return icons[platform] ?? `<svg viewBox="0 0 24 24" focusable="false"><rect x="3" y="4" width="18" height="13" rx="2" fill="none" stroke="currentColor" stroke-width="2"/><path d="M8 21h8m-4-4v4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>`;
}

function renderDashboard() {
  const manager = state.dashboard.manager;
  elements.version.textContent = manager.appVersion;
  elements.platform.textContent = `${manager.platform} / ${manager.arch}`;
  elements.platformIcon.innerHTML = managerPlatformIconMarkup(manager.platform);
  elements.managerIdentity.setAttribute("aria-label", `Manager version ${manager.appVersion} · ${manager.platform} / ${manager.arch}`);
  elements.catalogSource.textContent = state.dashboard.catalogSource.replace(/\+/g, " + ").replace(/-/g, " ");
  elements.updaterStatus.textContent = manager.updaterConfigured ? "Configured" : "Not configured";
  applyReleaseControlState();
  renderPlugins();
}

async function updateBetaReleasesPreference(enabled) {
  setBusy(true);
  try {
    hideAlert();
    await invoke("set_beta_releases_enabled", { enabled });
    logActivity(enabled ? "Beta releases enabled." : "Beta releases disabled.");
    await refreshDashboard();
  } catch (error) {
    elements.betaToggle.checked = !enabled;
    const parsed = parseUiError(error, "Couldn't update beta release settings.");
    showAlert(parsed);
    logActivity(`Beta release setting failed: ${parsed.summary}`);
  } finally {
    setBusy(false);
  }
}

async function updateDevelopmentBuildsPreference(enabled) {
  setBusy(true);
  try {
    hideAlert();
    await invoke("set_development_builds_enabled", { enabled });
    logActivity(enabled ? "Development builds enabled." : "Development builds disabled.");
    await refreshDashboard();
  } catch (error) {
    const parsed = parseUiError(error, "Couldn't update development build settings.");
    showAlert(parsed);
    logActivity(`Development build setting failed: ${parsed.summary}`);
  } finally {
    setBusy(false);
  }
}

async function activateDevelopmentReceiptsForCatalog() {
  const plugins = (state.dashboard?.plugins ?? [])
    .filter((plugin) =>
      plugin.accessMode === "licensed" && plugin.releaseChannel === "dev")
    .filter((plugin) => Boolean(plugin.pluginId));
  for (const plugin of plugins) {
    try {
      plugin.developmentReceiptStatus = await invoke("development_receipt_status", {
        productSlug: plugin.pluginId
      });
    } catch {
      plugin.developmentReceiptStatus = "unknown";
    }
    if (plugin.developmentReceiptStatus !== "active") {
      activatedDevelopmentProducts.delete(plugin.pluginId);
    }
  }
  const products = plugins.map((plugin) => plugin.pluginId);
  const availableProducts = new Set(products);
  for (const productSlug of activatedDevelopmentProducts) {
    if (!availableProducts.has(productSlug)) {
      activatedDevelopmentProducts.delete(productSlug);
    }
  }
  if (!state.dashboard?.manager?.developmentInvitationConnected) {
    activatedDevelopmentProducts.clear();
    return;
  }
  for (const productSlug of products) {
    if (activatedDevelopmentProducts.has(productSlug)) continue;
    try {
      await invoke("activate_development_access", { productSlug });
      activatedDevelopmentProducts.add(productSlug);
      const plugin = plugins.find((item) => item.pluginId === productSlug);
      if (plugin) plugin.developmentReceiptStatus = "active";
      logActivity(`Development receipt activated for ${productSlug}.`);
    } catch (error) {
      const parsed = parseUiError(error, `Development receipt is unavailable for ${productSlug}.`);
      logActivity(`Development receipt unavailable for ${productSlug}: ${parsed.summary}`);
    }
  }
}

async function connectDevelopmentInvitation() {
  if (!elements.developmentToken || !elements.developmentStatus) return;
  const token = elements.developmentToken.value.trim();
  if (!token) {
    elements.invitationConnectFeedback.textContent = "Enter an invitation code.";
    return;
  }
  elements.invitationConnectFeedback.textContent = "";
  elements.invitationConnectErrorDetails.classList.add("hidden");
  elements.invitationConnectErrorDetails.open = false;
  elements.invitationConnectErrorMessage.textContent = "";
  setBusy(true);
  try {
    hideAlert();
    const connectedEmail = await invoke("connect_development_invitation", { token });
    elements.developmentToken.value = "";
    state.invitation.status = "connected";
    if (typeof connectedEmail === "string" && connectedEmail) {
      state.invitation.email = connectedEmail;
    }
    state.invitation.receipt = "";
    invoke("forget_invitation_request_receipt").catch(() => {});
    persistInvitationMetadata();
    activatedDevelopmentProducts.clear();
    closeDialog(elements.invitationConnectDialog);
    logActivity("Development invitation connected securely.");
    await refreshDashboard();
  } catch (error) {
    elements.developmentToken.value = "";
    const parsed = parseUiError(error, "Couldn't connect the development invitation.");
    elements.invitationConnectFeedback.textContent = parsed.summary;
    if (parsed.details && parsed.details !== parsed.summary) {
      elements.invitationConnectErrorMessage.textContent = parsed.details;
      elements.invitationConnectErrorDetails.classList.remove("hidden");
      elements.invitationConnectErrorDetails.open = false;
    }
    logActivity(`Development invitation failed: ${parsed.summary}`);
  } finally {
    setBusy(false);
  }
}

async function forgetDevelopmentInvitation() {
  const invitationConnected = Boolean(state.dashboard?.manager?.developmentInvitationConnected);
  const requestAction = invitationRequestAction(state.invitation.status, state.invitation.receipt);
  if (!invitationConnected && requestAction === "withdraw") {
    await withdrawInvitationRequest();
    return;
  }
  if (!invitationConnected && requestAction === "forget-local") {
    const wasApproved = state.invitation.status === "approved_key_confirmation";
    clearInvitationMetadata();
    closeDialog(elements.invitationSettingsDialog);
    applyReleaseControlState();
    logActivity(wasApproved
      ? "Approved invitation request forgotten on this device."
      : "Invitation request forgotten on this device; the online request may still be active.");
    return;
  }
  setBusy(true);
  try {
    hideAlert();
    await invoke("forget_development_invitation");
    if (elements.developmentToken) elements.developmentToken.value = "";
    clearInvitationMetadata();
    closeDialog(elements.invitationSettingsDialog);
    logActivity("Development invitation removed.");
    await refreshDashboard();
  } catch (error) {
    const parsed = parseUiError(error, "Couldn't forget the development invitation.");
    showAlert(parsed);
    logActivity(`Forget invitation failed: ${parsed.summary}`);
  } finally {
    setBusy(false);
  }
}

function openDialog(dialog) {
  if (!dialog) return;
  if (dialog.open) dialog.close();
  dialog.showModal();
  syncAlertLayer();
}

function closeDialog(dialog) {
  if (dialog?.open) {
    dialog.close();
    syncAlertLayer();
  }
}

function setInvitationPending(response, email) {
  state.invitation = {
    status: response.verificationRequired ? "verification_required" : "pending",
    email: response.email || email || state.invitation.email,
    receipt: response.receipt || state.invitation.receipt,
    requestId: response.requestId == null ? state.invitation.requestId : String(response.requestId)
  };
  persistInvitationMetadata();
  if (state.invitation.receipt) {
    invoke("store_invitation_request_receipt", { receipt: state.invitation.receipt }).catch((error) => {
      logActivity(`Invitation request receipt could not be stored securely: ${String(error)}`);
    });
  }
  applyReleaseControlState();
}

function invitationResponseNeedsVerification(response) {
  return Boolean(response.verificationRequired || (response.requestId && !response.token && ["verify", "verification_required", "awaiting_verification"].includes(response.status)));
}

async function storeInvitationToken(token) {
  if (!token || !elements.developmentToken) return false;
  // The native command validates the token against /dev/catalog before keyring storage.
  elements.developmentToken.value = token;
  await connectDevelopmentInvitation();
  return true;
}

function openConnectInvitationDialog() {
  if (!elements.developmentToken || !elements.developmentStatus || !elements.invitationConnectDialog) return;
  elements.developmentToken.value = "";
  elements.developmentStatus.textContent = "";
  elements.invitationConnectFeedback.textContent = "";
  elements.invitationConnectErrorDetails.classList.add("hidden");
  elements.invitationConnectErrorDetails.open = false;
  elements.invitationConnectErrorMessage.textContent = "";
  openDialog(elements.invitationConnectDialog);
  window.setTimeout(() => elements.developmentToken.focus(), 0);
}

function openRequestInvitationDialog() {
  if (!elements.invitationRequestFeedback || !elements.invitationRequestDialog) return;
  elements.invitationRequestFeedback.textContent = "";
  elements.invitationRequestFeedback.classList.remove("success", "information");
  if (elements.invitationRequestAcknowledgement) elements.invitationRequestAcknowledgement.checked = false;
  if (elements.invitationRequestEmail && state.invitation.email) {
    elements.invitationRequestEmail.value = state.invitation.email;
  }
  openDialog(elements.invitationRequestDialog);
  window.setTimeout(() => elements.invitationRequestEmail?.focus(), 0);
}

async function submitInvitationRequest(event) {
  if (!event || !elements.invitationRequestForm || !elements.invitationRequestEmail) return;
  event.preventDefault();
  const email = elements.invitationRequestEmail.value.trim();
  const message = elements.invitationRequestMessage?.value.trim() || "";
  const acknowledged = elements.invitationRequestAcknowledgement?.checked === true;
  elements.invitationRequestMessage?.setCustomValidity(message ? "" : "Enter a short message about what you’d like to test.");
  if (!email || !message || !acknowledged) {
    elements.invitationRequestForm.reportValidity();
    if (elements.invitationRequestFeedback) {
      elements.invitationRequestFeedback.textContent = !email
        ? "Enter a valid email address."
        : !message
          ? "Add a short message about what you’d like to test."
          : "Confirm that you understand the review and feedback expectations.";
    }
    return;
  }
  if (!elements.invitationRequestForm.reportValidity()) return;
  const tracked = ["pending", "verification_required", "approved_key_confirmation"].includes(state.invitation.status);
  if (tracked && email.toLowerCase() === state.invitation.email.toLowerCase()) {
    if (state.invitation.status === "verification_required") {
      closeDialog(elements.invitationRequestDialog);
      openVerificationDialog();
      return;
    }
    if (state.invitation.receipt) await refreshInvitationRequestStatus({ feedbackInRequestDialog: true });
    if (state.invitation.status === "approved_key_confirmation") {
      setInvitationRequestFeedback("Approved. Check your email for your invitation code. Contact support@moazelgabry.com if it’s missing.");
    } else if (state.invitation.status === "pending") {
      setInvitationRequestFeedback("You already have a request under review. Its status is saved here; check it from the invitation panel.");
    }
    return;
  }
  if (tracked) {
    setInvitationRequestFeedback(`This manager is already tracking a request for ${state.invitation.email}. Check or finish that request before using another email.`);
    return;
  }
  setInvitationDialogBusy(elements.invitationRequestSubmit, true, "Sending request…");
  if (elements.invitationRequestFeedback) {
    elements.invitationRequestFeedback.textContent = "";
    elements.invitationRequestFeedback.classList.remove("success", "information");
  }
  try {
    const response = await invitationApi.request({
      email,
      pluginSlugs: [],
      message
    });
    setInvitationPending(response, email);
    closeDialog(elements.invitationRequestDialog);
    logActivity(`Invitation request sent for ${email}.`);
    if (invitationResponseNeedsVerification(response)) {
      openVerificationDialog();
    } else if (response.token) {
      await storeInvitationToken(response.token);
    } else {
      showAlert({
        summary: "Invitation request received.",
        details: "We’ll email you when the request is ready. You can check its status from the invitation panel."
      });
    }
  } catch (error) {
    const message = invitationServiceMessage(error);
    if (elements.invitationRequestFeedback) {
      elements.invitationRequestFeedback.classList.remove("success", "information");
      elements.invitationRequestFeedback.textContent = message;
    }
    logActivity(`Invitation request failed: ${message}`);
  } finally {
    setInvitationDialogBusy(elements.invitationRequestSubmit, false, "Send request");
  }
}

function setInvitationRequestFeedback(message, tone = "information") {
  if (!elements.invitationRequestFeedback) return;
  elements.invitationRequestFeedback.textContent = message;
  elements.invitationRequestFeedback.classList.remove("success", "information");
  if (tone) elements.invitationRequestFeedback.classList.add(tone);
}

function openVerificationDialog() {
  if (!elements.invitationVerifyDialog || !elements.invitationVerificationCode) return;
  if (elements.invitationVerifyEmail) elements.invitationVerifyEmail.textContent = state.invitation.email || "your email address";
  if (elements.invitationVerifyFeedback) elements.invitationVerifyFeedback.textContent = "";
  elements.invitationVerifyFeedback?.classList.remove("success", "information");
  elements.invitationVerificationCode.value = "";
  elements.invitationVerificationCode.disabled = false;
  if (elements.invitationVerifySubmit) elements.invitationVerifySubmit.textContent = "Verify email";
  openDialog(elements.invitationVerifyDialog);
  window.setTimeout(() => elements.invitationVerificationCode.focus(), 0);
}

async function submitInvitationVerification(event) {
  if (!event || !elements.invitationVerifyForm || !elements.invitationVerificationCode) return;
  event.preventDefault();
  if (elements.invitationVerifySubmit?.textContent === "Done") {
    closeDialog(elements.invitationVerifyDialog);
    return;
  }
  if (state.invitation.status === "pending" && state.invitation.receipt) {
    closeDialog(elements.invitationVerifyDialog);
    return;
  }
  const code = elements.invitationVerificationCode.value.trim();
  if (!code || !state.invitation.requestId) {
    if (elements.invitationVerifyFeedback) elements.invitationVerifyFeedback.textContent = "Enter the verification code from your email.";
    return;
  }
  setInvitationDialogBusy(elements.invitationVerifySubmit, true, "Verifying…");
  let verificationCompleted = false;
  try {
    const response = await invitationApi.verify({ requestId: state.invitation.requestId, code });
    if (response.alreadyApproved) {
      const email = state.invitation.email;
      state.invitation.status = "approved_key_confirmation";
      persistInvitationMetadata();
      applyReleaseControlState();
      verificationCompleted = true;
      elements.invitationVerificationCode.disabled = true;
      elements.invitationVerifyFeedback?.classList.remove("success");
      elements.invitationVerifyFeedback?.classList.add("information");
      if (elements.invitationVerifyFeedback) {
        elements.invitationVerifyFeedback.textContent = "Approved. Check your email for your invitation code; contact support@moazelgabry.com if it’s missing.";
      }
      logActivity(`Previously approved development access confirmed for ${email}.`);
      return;
    }
    const outcome = classifyInvitationVerificationResponse(response);
    if (outcome === "connected") {
      closeDialog(elements.invitationVerifyDialog);
      await storeInvitationToken(response.token);
      return;
    }
    if (outcome === "pending") {
      setInvitationPending(response, state.invitation.email);
      verificationCompleted = true;
      elements.invitationVerificationCode.disabled = true;
      elements.invitationVerifyFeedback?.classList.add("success");
      if (elements.invitationVerifyFeedback) {
        elements.invitationVerifyFeedback.textContent = "Email verified. Your request is awaiting review. We’ll email you when access is approved.";
      }
      logActivity(`Invitation request verified for ${state.invitation.email}.`);
      return;
    }
    if (elements.invitationVerifyFeedback) elements.invitationVerifyFeedback.textContent = "Your email was verified, but the service did not return a request receipt, so the manager cannot track the request. Check your email for updates or contact support@moazelgabry.com.";
  } catch (error) {
    const message = invitationServiceMessage(error);
    if (elements.invitationVerifyFeedback) elements.invitationVerifyFeedback.textContent = message;
    showAlert({ summary: "Invitation verification could not be completed.", details: message });
  } finally {
    setInvitationDialogBusy(elements.invitationVerifySubmit, false, verificationCompleted ? "Done" : "Verify email");
  }
}

function setInvitationDialogBusy(button, busy, label) {
  if (button) {
    button.disabled = busy;
    button.textContent = label;
  }
  [elements.invitationRequestCancel, elements.invitationRequestClose, elements.invitationVerifyCancel, elements.invitationVerifyClose].forEach((item) => {
    if (item) item.disabled = busy;
  });
}

function invitationServiceMessage(error) {
  if (error?.code === "moaz_er_invitation_cooldown") {
    const rawUntil = error.cooldownUntil;
    const date = rawUntil ? new Date(`${rawUntil.replace(" ", "T")}Z`) : null;
    if (date && !Number.isNaN(date.valueOf())) {
      const until = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date);
      const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
      return error.automaticCooldown
        ? `After five consecutive invitation requests, there is an automatic one-hour cooldown. You can request another invitation after ${until}${timeZone ? ` (${timeZone})` : ""}.`
        : `This email is on cooldown until ${until}${timeZone ? ` (${timeZone})` : ""}. You can request another invitation after that time.`;
    }
    return error.automaticCooldown
      ? "After five consecutive invitation requests, there is an automatic one-hour cooldown. Please try again after the cooldown ends."
      : "This email is on a temporary cooldown. You can request another invitation after the cooldown ends.";
  }
  if (error?.code === "moaz_er_invitation_blocked") {
    return "This email is blocked from new invitation requests. Contact support@moazelgabry.com if you believe this is a mistake.";
  }
  if (error?.status === 404) return "Invitation requests are not available yet. Please contact support@moazelgabry.com.";
  return error?.message || "The invitation service could not complete that request. Try again or contact support@moazelgabry.com.";
}

async function refreshInvitationRequestStatus({ feedbackInRequestDialog = false } = {}) {
  if (!state.invitation.receipt) return;
  state.invitationStatusRequest = invitationApi.status(state.invitation.receipt);
  try {
    const response = await state.invitationStatusRequest;
    const status = response.status;
    if (["approved", "verified", "connected", "fulfilled"].includes(status) && response.token) {
      await storeInvitationToken(response.token);
      return;
    }
    if (["approved", "fulfilled"].includes(status)) {
      state.invitation.status = "approved_key_confirmation";
      persistInvitationMetadata();
      applyReleaseControlState();
      logActivity("Invitation request approved; invitation code is available by email.");
      if (feedbackInRequestDialog) {
        setInvitationRequestFeedback("Approved. Check your email for the invitation code. Contact support@moazelgabry.com if it’s missing.");
      } else {
        showAlert({
          summary: "Invitation approved",
          type: "info",
          details: "Check your email for your invitation code. Contact support@moazelgabry.com if it’s missing."
        });
      }
      return;
    }
    if (["withdrawn", "cancelled", "canceled", "rejected"].includes(status)) {
      clearInvitationMetadata();
      if (feedbackInRequestDialog) setInvitationRequestFeedback("This request is no longer active. You can send a new request now.");
    } else {
      setInvitationPending(response, state.invitation.email);
      if (feedbackInRequestDialog) setInvitationRequestFeedback("You already have a request under review. Its status is saved here; check it from the invitation panel.");
    }
    applyReleaseControlState();
  } catch (error) {
    // A missing/network-unavailable status route must not disable public catalog use.
    logActivity(`Invitation status unavailable: ${invitationServiceMessage(error)}`);
    if (feedbackInRequestDialog) {
      setInvitationRequestFeedback("This manager is already tracking your request, but its status couldn’t be checked. Try again from the invitation panel.");
    }
  } finally {
    state.invitationStatusRequest = null;
  }
}

async function withdrawInvitationRequest() {
  if (!state.invitation.receipt) {
    showAlert({
      summary: "This request can’t be withdrawn here",
      details: "The secure request receipt is missing. Use Forget local request to remove it from this manager, or contact support@moazelgabry.com to cancel the online request."
    });
    return;
  }
  setBusy(true);
  try {
    await invitationApi.withdraw(state.invitation.receipt);
    clearInvitationMetadata();
    closeDialog(elements.invitationSettingsDialog);
    logActivity("Invitation request withdrawn.");
    applyReleaseControlState();
  } catch (error) {
    showAlert({ summary: "Couldn’t withdraw the invitation request.", details: invitationServiceMessage(error) });
  } finally {
    setBusy(false);
  }
}

function openInvitationSettings() {
  if (!elements.invitationSettingsDialog) return;
  const invitationConnected = Boolean(state.dashboard?.manager?.developmentInvitationConnected);
  const requestAction = invitationRequestAction(state.invitation.status, state.invitation.receipt);
  if (elements.developmentForget) {
    elements.developmentForget.textContent = requestAction === "forget-local"
      ? state.invitation.status === "approved_key_confirmation" ? "Forget approved request" : "Forget local request"
      : requestAction === "withdraw" ? "Withdraw request" : "Forget invitation";
  }
  elements.replaceInvitationButton?.classList.toggle("hidden", !invitationConnected);
  if (elements.invitationSettingsCopy) {
    elements.invitationSettingsCopy.textContent = invitationConnected
      ? "Manage your invitation access."
      : state.invitation.status === "verification_required"
        ? "Enter the verification code sent to your email to finish this request."
        : state.invitation.status === "approved_key_confirmation"
          ? "Your access is approved. Enter the emailed invitation code to connect it, or contact support@moazelgabry.com if it’s missing."
        : state.invitation.receipt
          ? "Your invitation request is pending. You can check its status or withdraw it here."
          : "The secure receipt is missing, so this request can’t be withdrawn here. Forgetting it only removes the local record; contact support to cancel it online.";
  }
  openDialog(elements.invitationSettingsDialog);
}

let activeLicenseProduct = null;

function openLicenseDialog(plugin) {
  if (!elements.licenseDialog) return;
  activeLicenseProduct = plugin;
  elements.licenseProductName.textContent = plugin.displayName;
  elements.licenseKey.value = "";
  elements.licenseKeyFeedback.textContent = "";
  openDialog(elements.licenseDialog);
  window.setTimeout(() => elements.licenseKey.focus(), 0);
}

function openManageAccessDialog(plugin) {
  if (!elements.accessDialog) return;
  const access = accessForProduct(plugin.pluginId);
  elements.accessDialogTitle.textContent = `${plugin.displayName} access`;
  const licenses = access?.licenses ?? [];
  elements.accessDialogBody.innerHTML = licenses.length
    ? licenses.map((license) => `
        <div class="access-source-row">
          <div>
            <strong>${escapeHtml(license.email || license.licenseId)}</strong>
            <small>${license.email ? `License #${escapeHtml(license.licenseId)} · ` : ""}${accessStatusLabel(license.status)}${accessExpiryLabel(license.expiresAt)}</small>
          </div>
          <button type="button" class="danger-button" data-manage-license-id="${escapeHtml(license.licenseId)}" data-manage-product="${escapeHtml(plugin.pluginId)}">Deactivate this product</button>
        </div>
      `).join("")
    : '<p class="empty-state">No license sources are stored for this product.</p>';
  elements.accessDialogBody.querySelectorAll("[data-manage-license-id]").forEach((button) => {
    button.addEventListener("click", async () => {
      if (!window.confirm(`Deactivate ${plugin.displayName} from this device for this license source?`)) return;
      button.disabled = true;
      try {
        state.access = await invoke("deactivate_access", {
          licenseId: button.dataset.manageLicenseId,
          productSlug: button.dataset.manageProduct
        });
        closeDialog(elements.accessDialog);
        renderPlugins();
        logActivity(`${plugin.displayName}: product access deactivated on this device.`);
      } catch (error) {
        button.disabled = false;
        const parsed = parseUiError(error, `Couldn’t deactivate ${plugin.displayName} access.`);
        showAlert(parsed);
      }
    });
  });
  openDialog(elements.accessDialog);
}

async function submitLicenseActivation(event) {
  event?.preventDefault();
  const key = elements.licenseKey?.value.trim();
  const plugin = activeLicenseProduct;
  if (!key || !plugin) {
    elements.licenseKeyFeedback.textContent = "Enter a license key.";
    return;
  }
  elements.licenseKeyFeedback.textContent = "Activating this key across its product entitlements…";
  elements.licenseForm.querySelector('[type="submit"]').disabled = true;
  try {
    const result = await invoke("activate_access", { key, productSlug: plugin.pluginId });
    state.access = result.state;
    const summary = result.products.map((product) => `${product.productSlug}: ${accessStatusLabel(product.status)}`).join("; ");
    closeDialog(elements.licenseDialog);
    renderPlugins();
    logActivity(`License activated. ${summary || "No product entitlements were returned."}`);
  } catch (error) {
    const parsed = parseUiError(error, "Couldn’t activate this license key.");
    elements.licenseKeyFeedback.textContent = parsed.summary;
    showAlert(parsed);
    logActivity(`License activation failed: ${parsed.summary}`);
  } finally {
    elements.licenseForm.querySelector('[type="submit"]').disabled = false;
  }
}

async function refreshLicenseAccess() {
  const products = (state.dashboard?.plugins ?? [])
    .filter((plugin) => plugin.accessMode === "licensed" && plugin.releaseChannel !== "dev")
    .map((plugin) => plugin.pluginId);
  try {
    state.access = await invoke("refresh_access", { productSlugs: products });
  } catch (error) {
    logActivity(`License refresh unavailable: ${parseUiError(error, "License access is unavailable.").summary}`);
  }
}

async function refreshDashboard() {
  startOperation("catalog", null, "Refreshing plugin catalog");
  setBusy(true);
  try {
    hideAlert();
    state.dashboard = await invoke("dashboard_state");
    await activateDevelopmentReceiptsForCatalog();
    try {
      state.access = await invoke("access_state");
    } catch (error) {
      state.access = { deviceId: "", products: [] };
      logActivity(`License access status is unavailable: ${parseUiError(error, "License access is unavailable.").summary}`);
    }
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

function shouldAutoCheckManagerUpdateForPluginAction(action) {
  return ["install", "update", "reinstall", "install-selected"].includes(action);
}

function managerUpdateCheckOptions() {
  return state.dashboard?.manager?.platform === "macos"
    ? { target: "darwin-universal" }
    : undefined;
}

async function runManagerUpdateCheck({ silent = false } = {}) {
  if (!state.dashboard?.manager?.updaterConfigured) {
    return { updated: false, error: null, skipped: true };
  }

  try {
    const update = await check(managerUpdateCheckOptions());
    if (!update) {
      return { updated: false, error: null, skipped: false };
    }

    if (!silent) {
      logActivity(`Downloading manager update ${update.version}.`);
    }
    await update.downloadAndInstall();
    if (!silent) {
      logActivity("Manager update installed. Restarting...");
    }
    await relaunch();
    return { updated: true, error: null, skipped: false };
  } catch (error) {
    const parsed = parseUiError(error, "Manager update failed.");
    return { updated: false, error: parsed, skipped: false };
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
  let deferredManagerUpdateError = null;
  try {
    hideAlert();
    if (shouldAutoCheckManagerUpdateForPluginAction(action)) {
      logActivity(`Checking for manager updates before ${action.replace("-", " ")}.`);
      updateOperationProgress({
        label: "Checking manager updates first",
        steps: ["Checking for manager updates", "Continuing with plugin install"],
        stepIndex: 0
      });
      const managerUpdate = await runManagerUpdateCheck();
      if (managerUpdate.error) {
        deferredManagerUpdateError = managerUpdate.error;
        logActivity(
          `Manager auto-update skipped before ${action.replace("-", " ")}: ${managerUpdate.error.summary}`
        );
      }
      updateOperationProgress({
        label: activeLabel,
        steps: operationSteps(action.includes("uninstall") ? "plugin-uninstall" : "plugin"),
        stepIndex: 0
      });
    }

    const result = await invoke("apply_plugin_action", { pluginId, action, targetVersion });
    logActivity(`${result.pluginId}: ${result.message}`);
    await refreshDashboard();
    if (deferredManagerUpdateError) {
      showAlert(deferredManagerUpdateError);
    }
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
  if (!state.dashboard?.manager) {
    logActivity("Manager updater status is unavailable until the catalog loads successfully.");
    return;
  }

  if (!state.dashboard?.manager?.updaterConfigured) {
    logActivity("Manager updater is not configured in this build yet.");
    return;
  }

  startOperation("manager-update", null, "Updating manager");
  setBusy(true);
  try {
    const outcome = await runManagerUpdateCheck();
    if (!outcome.updated && !outcome.error) {
      logActivity("Manager app is already up to date.");
      return;
    }
    if (outcome.error) {
      showAlert(outcome.error);
      logActivity(`Manager update failed: ${outcome.error.summary}`);
    }
  } finally {
    finishOperation();
    setBusy(false);
  }
}

async function exportPluginLogs(pluginId) {
  const plugin = state.dashboard?.plugins?.find((item) => item.pluginId === pluginId);
  const displayName = plugin?.displayName ?? pluginId;

  try {
    await invoke("check_plugin_log_export_ready", { pluginId });
  } catch (error) {
    const parsed = parseUiError(error, `Couldn't start diagnostics logs for ${displayName}.`);
    showAlert(parsed);
    logActivity(`${pluginId}: ${parsed.summary}`);
    return;
  }

  let diagnosticsOptions = null;
  try {
    diagnosticsOptions = await showDiagnosticsExportDialog(displayName);
  } catch (error) {
    const parsed = parseUiError(error, "Couldn't show the diagnostics instructions.");
    showAlert(parsed);
    logActivity(`${pluginId}: ${parsed.summary}`);
    return;
  }

  if (!diagnosticsOptions) {
    return;
  }

  let destinationDir = null;
  try {
    destinationDir = await open({
      directory: true,
      multiple: false,
      title: "Choose where to export plugin logs"
    });
  } catch (error) {
    const parsed = parseUiError(error, "Couldn't open the folder picker.");
    showAlert(parsed);
    logActivity(`${pluginId}: ${parsed.summary}`);
    return;
  }

  if (!destinationDir) {
    return;
  }

  startOperation("plugin-logs", pluginId, "Launching Resolve for diagnostics");
  setBusy(true);
  try {
    hideAlert();
    const cleanupNote = diagnosticsOptions.removePreviousLogs ? " Previous logs will be removed first." : "";
    logActivity(`${pluginId}: Starting diagnostics session.${cleanupNote} Close Resolve when you finish reproducing the issue.`);
    const result = await invoke("export_plugin_logs", {
      pluginId,
      destinationDir,
      removePreviousLogs: diagnosticsOptions.removePreviousLogs
    });
    logActivity(`${result.pluginId}: ${result.message}`);
    await refreshDashboard();
  } catch (error) {
    const parsed = parseUiError(error, `Couldn't export diagnostics logs for ${pluginId}.`);
    showAlert(parsed);
    logActivity(`${pluginId}: ${parsed.summary}`);
  } finally {
    finishOperation();
    setBusy(false);
  }
}

async function openSupportLink() {
  try {
    await invoke("open_support_link");
  } catch (error) {
    const parsed = parseUiError(error, "Couldn't open the support link.");
    showAlert(parsed);
    logActivity(`Support link failed: ${parsed.summary}`);
  }
}

elements.refreshButton.addEventListener("click", refreshDashboard);
elements.updateButton.addEventListener("click", checkForManagerUpdates);
elements.supportButton.addEventListener("click", openSupportLink);
elements.alertDismiss.addEventListener("click", hideAlert);
invitationDialogs.forEach((dialog) => dialog.addEventListener("close", syncAlertLayer));
elements.betaToggle.addEventListener("change", (event) => {
  updateBetaReleasesPreference(event.currentTarget.checked);
});
elements.developmentToggle.addEventListener("change", (event) => {
  updateDevelopmentBuildsPreference(event.currentTarget.checked);
});
bindEvent(elements.haveInvitationButton, "click", openConnectInvitationDialog);
bindEvent(elements.getInvitationButton, "click", openRequestInvitationDialog);
bindEvent(elements.invitationSettingsButton, "click", openInvitationSettings);
bindEvent(elements.checkInvitationStatusButton, "click", refreshInvitationRequestStatus);
bindEvent(elements.connectApprovedInvitationButton, "click", openConnectInvitationDialog);
bindEvent(elements.continueInvitationVerificationButton, "click", openVerificationDialog);
bindEvent(elements.withdrawInvitationButton, "click", withdrawInvitationRequest);
bindEvent(elements.invitationConnectForm, "submit", (event) => {
  event.preventDefault();
  connectDevelopmentInvitation();
});
bindEvent(elements.invitationRequestForm, "submit", submitInvitationRequest);
bindEvent(elements.invitationRequestMessage, "input", () => {
  if (elements.invitationRequestMessage?.value.trim()) {
    elements.invitationRequestMessage.setCustomValidity("");
  }
});
bindEvent(elements.invitationVerifyForm, "submit", submitInvitationVerification);
bindEvent(elements.invitationConnectClose, "click", () => closeDialog(elements.invitationConnectDialog));
bindEvent(elements.invitationConnectCancel, "click", () => closeDialog(elements.invitationConnectDialog));
bindEvent(elements.invitationRequestClose, "click", () => closeDialog(elements.invitationRequestDialog));
bindEvent(elements.invitationRequestCancel, "click", () => closeDialog(elements.invitationRequestDialog));
bindEvent(elements.invitationVerifyClose, "click", () => closeDialog(elements.invitationVerifyDialog));
bindEvent(elements.invitationVerifyCancel, "click", () => closeDialog(elements.invitationVerifyDialog));
bindEvent(elements.invitationSettingsClose, "click", () => closeDialog(elements.invitationSettingsDialog));
bindEvent(elements.replaceInvitationButton, "click", () => {
  closeDialog(elements.invitationSettingsDialog);
  openConnectInvitationDialog();
});
bindEvent(elements.developmentForget, "click", forgetDevelopmentInvitation);
bindEvent(elements.licenseForm, "submit", submitLicenseActivation);
bindEvent(elements.licenseDialogClose, "click", () => closeDialog(elements.licenseDialog));
bindEvent(elements.licenseDialogCancel, "click", () => closeDialog(elements.licenseDialog));
bindEvent(elements.accessDialogClose, "click", () => closeDialog(elements.accessDialog));
elements.releaseHighlightsClose.addEventListener("click", closeReleaseHighlightsDialog);
elements.releaseHighlightsDialog.addEventListener("cancel", (event) => {
  event.preventDefault();
  closeReleaseHighlightsDialog();
});
elements.releaseHighlightsDialog.addEventListener("click", (event) => {
  if (event.target === elements.releaseHighlightsDialog) {
    closeReleaseHighlightsDialog();
  }
});

listen("plugin-log-export-progress", (event) => {
  const progress = event.payload;
  if (
    !progress ||
    state.activeOperation?.kind !== "plugin-logs" ||
    state.activeOperation.pluginId !== progress.pluginId
  ) {
    return;
  }

  updateOperationProgress({
    label: progress.label,
    steps: progress.steps,
    stepIndex: progress.stepIndex
  });
}).catch((error) => {
  logActivity(`Diagnostics progress updates unavailable: ${String(error)}`);
});

hydrateInvitationReceipt().finally(async () => {
  await refreshDashboard();
  await refreshLicenseAccess();
  renderPlugins();
  if (["pending", "approved_key_confirmation"].includes(state.invitation.status) && state.invitation.receipt) {
    refreshInvitationRequestStatus();
  }
});
