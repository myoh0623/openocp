import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";
import { applyTranslations, normalizeLanguage, translate } from "./i18n.js";
import { version as appVersion } from "../package.json";

const APP_LANGUAGE_KEY = "remoteOcp.appLanguage";
let appLanguage = normalizeLanguage(localStorage.getItem(APP_LANGUAGE_KEY));
const t = (key, values) => translate(appLanguage, key, values);
const PROFILE_KEY = "remoteOcp.connectionProfiles.v2";
const LAST_PROFILE_KEY = "remoteOcp.lastProfile.v2";
const fieldIds = [
  "connectionMode", "profileName", "sshIP", "sshUser", "dashboardURL", "sshPort", "socksPort",
  "browserLanguage", "browserPath", "browserProfilePath", "privateKeyPath",
  "keepTemporaryProfile", "insecureTLS", "remoteHost", "remoteUser", "remotePort", "remoteFolder", "vscodePath"
];
const ephemeralIds = ["password", "privateKeyPassphrase", "rhtlcURL"];
const $ = (id) => document.getElementById(id);
let profiles = loadProfiles();
let selectedProfileId = "";
let connected = false;
let connecting = false;
let disconnectRequested = false;
let browserSessionCount = 0;
let toastTimer;
let connectionState = "idle";
let activeSocksPort = null;
let hasLog = false;
const openingTools = new Set();
let connectionAttempt = 0;

function loadProfiles() {
  try {
    const parsed = JSON.parse(localStorage.getItem(PROFILE_KEY) || "[]");
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function persistProfiles() {
  profiles.sort((a, b) => a.name.localeCompare(b.name, appLanguage, { sensitivity: "base" }));
  localStorage.setItem(PROFILE_KEY, JSON.stringify(profiles));
}

function fieldValue(id) {
  const element = $(id);
  return element.type === "checkbox" ? element.checked : element.value.trim();
}

function setField(id, value) {
  const element = $(id);
  if (element.type === "checkbox") element.checked = Boolean(value);
  else element.value = value ?? "";
}

function currentProfile(id = selectedProfileId || crypto.randomUUID()) {
  const profile = { id };
  for (const field of fieldIds) profile[field] = fieldValue(field);
  profile.name = profile.profileName || t("profileDefaultName", { number: profiles.length + 1 });
  delete profile.profileName;
  return profile;
}

function refreshProfileSelect() {
  const select = $("profileSelect");
  select.replaceChildren(new Option(t("newProfile"), ""));
  for (const profile of profiles) select.add(new Option(profile.name, profile.id));
  select.value = selectedProfileId;
  $("deleteProfileButton").disabled = connecting || connected || !selectedProfileId;
}

function applyProfile(profile) {
  selectedProfileId = profile.id;
  setField("profileName", profile.name);
  for (const field of fieldIds.filter((name) => name !== "profileName")) {
    const defaults = { browserLanguage: "en-US", connectionMode: "ssh", remoteUser: "student", remotePort: "22" };
    const value = profile[field] ?? defaults[field];
    setField(field, value);
  }
  for (const field of ephemeralIds) setField(field, "");
  localStorage.setItem(LAST_PROFILE_KEY, profile.id);
  refreshProfileSelect();
  showProfileMessage(t("profileLoaded", { name: profile.name }));
  validate();
}

function resetForm() {
  selectedProfileId = "";
  for (const id of fieldIds) setField(id, "");
  setField("connectionMode", "ssh");
  setField("sshUser", "student");
  setField("sshPort", "22");
  setField("remoteUser", "student");
  setField("remotePort", "22");
  setField("socksPort", "1080");
  setField("browserLanguage", "en-US");
  for (const id of ephemeralIds) setField(id, "");
  refreshProfileSelect();
  showProfileMessage(t("profileNewHint"));
  validate();
  $("sshIP").focus();
}

function saveProfile(silent = false) {
  if (connecting || connected) return false;
  const issue = validate(true);
  if (issue) {
    if (!silent) showToast(issue);
    return false;
  }
  const id = selectedProfileId || crypto.randomUUID();
  const profile = currentProfile(id);
  const index = profiles.findIndex((item) => item.id === id);
  if (index >= 0) profiles[index] = profile;
  else profiles.push(profile);
  selectedProfileId = id;
  persistProfiles();
  localStorage.setItem(LAST_PROFILE_KEY, id);
  refreshProfileSelect();
  showProfileMessage(t("profileSaved", { name: profile.name }));
  return true;
}

function deleteSelectedProfile() {
  if (connecting || connected || !selectedProfileId) return;
  const profile = profiles.find((item) => item.id === selectedProfileId);
  $("confirmText").textContent = t("deleteConfirm", { name: profile?.name || t("selected") });
  $("confirmDialog").showModal();
}

function validate(forSave = false) {
  const rhtlc = $("connectionMode").value === "rhtlc";
  $("sshFields").classList.toggle("hidden", rhtlc);
  $("rhtlcFields").classList.toggle("hidden", !rhtlc);
  $("privateKeyPassphraseField").classList.toggle("hidden", rhtlc);
  $("remoteTargetFields").classList.toggle("hidden", !rhtlc);
  $("developerToolsHint").textContent = t(rhtlc ? "toolsHintRhtlc" : "toolsHintSSH");
  $("dashboardHint").textContent = rhtlc
    ? t("dashboardHintRhtlc")
    : t("dashboardHintSSH");
  const ip = $("sshIP").value.trim();
  const user = $("sshUser").value.trim();
  const sshPort = Number($("sshPort").value);
  const socksPort = Number($("socksPort").value);
  const url = $("dashboardURL").value.trim();
  const browserLanguage = $("browserLanguage").value;
  let issue = "";
  const octets = ip.split(".");
  if (!rhtlc && (octets.length !== 4 || octets.some((part) => !/^\d{1,3}$/.test(part) || Number(part) > 255))) {
    issue = t("invalidIP");
  } else if (!rhtlc && !/^[A-Za-z0-9._][A-Za-z0-9._-]{0,127}$/.test(user)) {
    issue = t("invalidUser");
  } else if (!rhtlc && (!Number.isInteger(sshPort) || sshPort < 1 || sshPort > 65535)) {
    issue = t("invalidSSHPort");
  } else if (!Number.isInteger(socksPort) || socksPort < 1024 || socksPort > 65515) {
    issue = t("invalidSocksPort");
  } else if (!["en-US", "ko-KR"].includes(browserLanguage)) {
    issue = t("chooseBrowserLanguage");
  } else if (url) {
    try {
      const parsed = new URL(url);
      if (!["http:", "https:"].includes(parsed.protocol)) throw new Error();
    } catch {
      issue = t("invalidDashboard");
    }
  }
  if (!issue && rhtlc && fieldValue("remoteHost")) {
    const host = fieldValue("remoteHost");
    let validHost = /^[A-Za-z0-9][A-Za-z0-9._-]{0,252}$/.test(host);
    if (host.includes(":")) {
      try { validHost = new URL(`http://[${host}]/`).hostname.startsWith("["); }
      catch { validHost = false; }
    }
    if (!validHost) issue = t("invalidRemoteHost");
    else if (!/^[A-Za-z0-9._][A-Za-z0-9._-]{0,127}$/.test(fieldValue("remoteUser"))) issue = t("invalidUser");
    else if (!Number.isInteger(Number(fieldValue("remotePort"))) || Number(fieldValue("remotePort")) < 1 || Number(fieldValue("remotePort")) > 65535) issue = t("invalidSSHPort");
  }
  const folder = fieldValue("remoteFolder");
  if (!issue && folder && (!folder.startsWith("/") || /[\x00-\x1f\x7f]/.test(folder))) issue = t("invalidRemoteFolder");
  const profileIssue = issue;
  if (!issue && rhtlc && !connected && !connecting) {
    try {
      const input = fieldValue("rhtlcURL");
      if (!input.startsWith("rhtlc://") || /[\x00-\x1f\x7f]/.test(input)) throw new Error();
      const parsed = new URL(input.replace(/^rhtlc:\/\/(?:https:\/\/|wss:\/\/)?/, "wss://"));
      if (parsed.protocol !== "wss:" || !parsed.hostname || !parsed.searchParams.get("token") || parsed.username || parsed.password || parsed.hash) throw new Error();
    } catch {
      issue = t("invalidRhtlc");
    }
  }
  $("validationMessage").textContent = issue;
  const locked = connecting || connected;
  for (const id of [...fieldIds, ...ephemeralIds, "profileSelect", "newProfileButton"]) {
    $(id).disabled = locked;
  }
  $("detectBrowserButton").disabled = locked;
  $("connectButton").disabled = Boolean(issue) || locked;
  $("saveProfileButton").disabled = Boolean(profileIssue) || locked;
  $("deleteProfileButton").disabled = locked || !selectedProfileId;
  updateToolButtons();
  updateTarget();
  return forSave ? profileIssue : issue;
}

function buildConnectionConfig() {
  return {
    connectionMode: fieldValue("connectionMode"),
    rhtlcUrl: fieldValue("rhtlcURL"),
    sshIp: fieldValue("sshIP"),
    sshPort: Number(fieldValue("sshPort")),
    sshUser: fieldValue("sshUser"),
    password: $("password").value,
    privateKeyPath: fieldValue("privateKeyPath"),
    privateKeyPassphrase: $("privateKeyPassphrase").value,
    firstSocksPort: Number(fieldValue("socksPort")),
    dashboardUrl: fieldValue("dashboardURL"),
    browserPath: fieldValue("browserPath"),
    browserProfilePath: fieldValue("browserProfilePath"),
    browserLanguage: fieldValue("browserLanguage"),
    keepTemporaryProfile: fieldValue("keepTemporaryProfile"),
    insecureTls: fieldValue("insecureTLS"),
    remoteHost: fieldValue("remoteHost"),
    remoteUser: fieldValue("remoteUser"),
    remotePort: Number(fieldValue("remotePort") || 22),
    remoteFolder: fieldValue("remoteFolder"),
    vscodePath: fieldValue("vscodePath")
  };
}

async function connect() {
  if (connecting || connected) return;
  if (validate()) return;
  const attempt = ++connectionAttempt;
  if ($("profileName").value.trim()) saveProfile(true);
  disconnectRequested = false;
  setConnectionState("connecting", t("connecting"));
  clearLog();
  appendLog(t("preparingLog"));
  try {
    const result = await invoke("start_connection", { config: buildConnectionConfig() });
    if (attempt !== connectionAttempt) return;
    $("password").value = "";
    $("privateKeyPassphrase").value = "";
    $("rhtlcURL").value = "";
    if (disconnectRequested) {
      await invoke("stop_connection");
      return;
    }
    activeSocksPort = result.socksPort;
    connected = true;
    connecting = false;
    setConnectionState("connected", t("connected", { port: result.socksPort }));
    appendLog(t("connectionLifetime"));
  } catch (error) {
    if (attempt !== connectionAttempt) return;
    connected = false;
    connecting = false;
    if (disconnectRequested) {
      setConnectionState("idle", t("disconnected"));
      appendLog(t("cancelled"));
      return;
    }
    setConnectionState("error", t("connectionFailed"));
    const message = String(error || t("unknownError"));
    appendLog(t("error", { message }));
    showToast(message);
    $("logPanel").open = true;
  }
}

async function disconnect() {
  disconnectRequested = true;
  updateToolButtons();
  $("disconnectButton").disabled = true;
  $("statusText").textContent = t("disconnecting");
  try { await invoke("stop_connection"); }
  catch (error) { showToast(String(error)); }
}

function updateToolButtons() {
  const hasTarget = fieldValue("connectionMode") === "ssh" || Boolean(fieldValue("remoteHost"));
  $("connectedTools").classList.toggle("hidden", !connected);
  $("toolsAvailability").textContent = hasTarget ? t("externalAuthHint") : t("toolsTargetRequired");
  for (const [id, tool] of [["openSessionButton", "browser"], ["terminalButton", "terminal"], ["vscodeButton", "vscode"]]) {
    $(id).disabled = !connected || disconnectRequested || openingTools.has(tool) || (tool !== "browser" && !hasTarget);
  }
}

async function openTool(tool) {
  if (!connected || disconnectRequested || openingTools.has(tool)) return;
  openingTools.add(tool);
  updateToolButtons();
  try {
    if (tool === "browser") await invoke("open_browser_session");
    else await invoke("open_external_tool", { tool });
    if (connected && !disconnectRequested) showToast(t(tool === "browser" ? "sessionOpened" : "toolOpened"));
  } catch (error) {
    showToast(String(error));
  } finally {
    openingTools.delete(tool);
    updateToolButtons();
  }
}

async function refreshBrowserDetection(showResultToast = false) {
  const hint = $("browserDetection");
  if (!window.__TAURI_INTERNALS__) {
    hint.textContent = t("detectBrowserPreview");
    hint.className = "field-hint";
    return;
  }

  hint.textContent = t("detectingBrowser");
  hint.className = "field-hint";
  try {
    const browser = await invoke("detect_browser", { requested: fieldValue("browserPath") });
    hint.textContent = `${browser.name} · ${browser.path}`;
    hint.className = "field-hint success";
    if (showResultToast) showToast(t("browserAvailable", { name: browser.name }));
  } catch (error) {
    hint.textContent = String(error);
    hint.className = "field-hint error";
    if (showResultToast) showToast(String(error));
  }
}

async function refreshRhtlcDetection() {
  const hint = $("rhtlcDetection");
  if (!window.__TAURI_INTERNALS__) {
    hint.textContent = t("rhtlcPreview");
    return;
  }
  try {
    await invoke("detect_rhtlc");
    hint.textContent = t("rhtlcReady");
    hint.className = "field-hint success";
  } catch (error) {
    hint.textContent = String(error);
    hint.className = "field-hint error";
  }
}

function setConnectionState(state, text) {
  connectionState = state;
  connecting = state === "connecting";
  connected = state === "connected";
  $("statusIcon").className = `status-icon ${state === "idle" ? "" : state}`;
  $("statusText").textContent = text;
  $("connectButton").classList.toggle("hidden", connecting || connected);
  $("disconnectButton").classList.toggle("hidden", !connecting && !connected);
  $("disconnectButton").disabled = false;
  document.querySelector(".app-shell").classList.toggle("busy", connecting || connected);
  if (state === "idle" || state === "error") browserSessionCount = 0;
  validate();
}

function updateTarget() {
  const ip = $("sshIP").value.trim();
  const user = $("sshUser").value.trim() || "student";
  const sessions = connected && browserSessionCount ? t("browserCount", { count: browserSessionCount }) : "";
  $("statusTarget").textContent = $("connectionMode").value === "rhtlc"
    ? t("rhtlcTarget", { sessions })
    : ip ? `${user}@${ip}${sessions}` : t("enterIP");
}

function appendLog(text) {
  const output = $("logOutput");
  if (!hasLog) output.textContent = "";
  hasLog = true;
  const time = new Intl.DateTimeFormat(appLanguage, { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false }).format(new Date());
  output.textContent += `[${time}] ${text}\n`;
  if (output.textContent.length > 80000) output.textContent = output.textContent.slice(-60000);
  output.scrollTop = output.scrollHeight;
}

function clearLog() { hasLog = false; $("logOutput").textContent = t("noLogs"); }

function showProfileMessage(message) {
  $("profileMessage").textContent = message;
  window.setTimeout(() => {
    if ($("profileMessage").textContent === message) $("profileMessage").textContent = "";
  }, 3500);
}

function showToast(message) {
  const toast = $("toast");
  toast.textContent = message;
  toast.classList.remove("hidden");
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => toast.classList.add("hidden"), 6000);
}

function renderLanguage() {
  document.documentElement.lang = appLanguage;
  $("appLanguage").value = appLanguage;
  applyTranslations(document, appLanguage);
  refreshProfileSelect();
  if (!hasLog) clearLog();
  $("profileMessage").textContent = "";
  $("toast").classList.add("hidden");
  const status = disconnectRequested && (connected || connecting)
    ? t("disconnecting")
    : connected ? t("connected", { port: activeSocksPort })
    : connecting ? t("connecting")
    : connectionState === "error" ? t("connectionFailed") : t("disconnected");
  $("statusText").textContent = status;
  validate();
}

async function changeAppLanguage() {
  const select = $("appLanguage");
  const next = normalizeLanguage(select.value);
  select.disabled = true;
  try {
    if (window.__TAURI_INTERNALS__) await invoke("set_app_language", { language: next });
    appLanguage = next;
    localStorage.setItem(APP_LANGUAGE_KEY, next);
    renderLanguage();
    await refreshBrowserDetection();
    if (fieldValue("connectionMode") === "rhtlc") await refreshRhtlcDetection();
  } catch (error) {
    select.value = appLanguage;
    showToast(String(error));
  } finally {
    select.disabled = false;
  }
}

async function initialize() {
  if (window.__TAURI_INTERNALS__) await invoke("set_app_language", { language: appLanguage });
  document.documentElement.lang = appLanguage;
  $("appLanguage").value = appLanguage;
  applyTranslations(document, appLanguage);
  clearLog();
  $("statusText").textContent = t("disconnected");
  const platform = navigator.userAgent.includes("Windows") ? "Windows" : navigator.userAgent.includes("Mac") ? "macOS" : "Desktop";
  $("platformBadge").textContent = `${platform} · v${appVersion.replace(/\.0$/, "")}`;
  refreshProfileSelect();
  const lastId = localStorage.getItem(LAST_PROFILE_KEY);
  const profile = profiles.find((item) => item.id === lastId) || profiles[0];
  if (profile) applyProfile(profile);
  else resetForm();

  if (!window.__TAURI_INTERNALS__) {
    await refreshBrowserDetection();
    return;
  }

  await listen("tunnel-log", (event) => appendLog(event.payload));
  await listen("browser-session-count", (event) => {
    browserSessionCount = Number(event.payload?.count || 0);
    updateTarget();
  });
  await listen("tunnel-status", (event) => {
    const { state, message } = event.payload;
    if (state === "stopped") {
      connectionAttempt++;
      setConnectionState("idle", t("disconnected"));
    }
    else if (state === "error") {
      connectionAttempt++;
      setConnectionState("error", t("connectionFailed"));
      showToast(message);
      $("logPanel").open = true;
    } else if (state === "connecting") setConnectionState("connecting", message);
  });
  await refreshBrowserDetection();
  if (fieldValue("connectionMode") === "rhtlc") await refreshRhtlcDetection();
}

$("profileSelect").addEventListener("change", (event) => {
  if (connecting || connected) return;
  const profile = profiles.find((item) => item.id === event.target.value);
  if (profile) applyProfile(profile);
  else resetForm();
  void refreshBrowserDetection();
});
$("newProfileButton").addEventListener("click", () => {
  if (!connecting && !connected) {
    resetForm();
    void refreshBrowserDetection();
  }
});
$("saveProfileButton").addEventListener("click", () => saveProfile(false));
$("deleteProfileButton").addEventListener("click", deleteSelectedProfile);
$("confirmDialog").addEventListener("close", () => {
  if ($("confirmDialog").returnValue !== "confirm") return;
  profiles = profiles.filter((item) => item.id !== selectedProfileId);
  persistProfiles();
  localStorage.removeItem(LAST_PROFILE_KEY);
  resetForm();
  void refreshBrowserDetection();
  showProfileMessage(t("profileDeleted"));
});
$("appLanguage").addEventListener("change", changeAppLanguage);
$("connectButton").addEventListener("click", connect);
$("openSessionButton").addEventListener("click", () => openTool("browser"));
$("terminalButton").addEventListener("click", () => openTool("terminal"));
$("vscodeButton").addEventListener("click", () => openTool("vscode"));
$("disconnectButton").addEventListener("click", disconnect);
$("detectBrowserButton").addEventListener("click", () => refreshBrowserDetection(true));
$("connectionMode").addEventListener("change", () => {
  validate();
  if (fieldValue("connectionMode") === "rhtlc") void refreshRhtlcDetection();
});
$("clearLogButton").addEventListener("click", clearLog);
for (const id of [...fieldIds, ...ephemeralIds]) $(id).addEventListener("input", validate);
$("browserPath").addEventListener("input", () => {
  $("browserDetection").textContent = t("checkPathHint");
  $("browserDetection").className = "field-hint";
});

initialize().catch((error) => showToast(t("initializationError", { message: error })));
