import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";
import { version as appVersion } from "../package.json";

const PROFILE_KEY = "remoteOcp.connectionProfiles.v2";
const LAST_PROFILE_KEY = "remoteOcp.lastProfile.v2";
const fieldIds = [
  "connectionMode", "profileName", "sshIP", "sshUser", "dashboardURL", "sshPort", "socksPort",
  "browserLanguage", "browserPath", "browserProfilePath", "privateKeyPath",
  "keepTemporaryProfile", "insecureTLS"
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

function loadProfiles() {
  try {
    const parsed = JSON.parse(localStorage.getItem(PROFILE_KEY) || "[]");
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function persistProfiles() {
  profiles.sort((a, b) => a.name.localeCompare(b.name, "ko", { sensitivity: "base" }));
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
  profile.name = profile.profileName || `프로필 ${profiles.length + 1}`;
  delete profile.profileName;
  return profile;
}

function refreshProfileSelect() {
  const select = $("profileSelect");
  select.replaceChildren(new Option("새 프로필", ""));
  for (const profile of profiles) select.add(new Option(profile.name, profile.id));
  select.value = selectedProfileId;
  $("deleteProfileButton").disabled = connecting || connected || !selectedProfileId;
}

function applyProfile(profile) {
  selectedProfileId = profile.id;
  setField("profileName", profile.name);
  for (const field of fieldIds.filter((name) => name !== "profileName")) {
    const value = field === "browserLanguage" ? (profile[field] || "en-US") : field === "connectionMode" ? (profile[field] || "ssh") : profile[field];
    setField(field, value);
  }
  for (const field of ephemeralIds) setField(field, "");
  localStorage.setItem(LAST_PROFILE_KEY, profile.id);
  refreshProfileSelect();
  showProfileMessage(`‘${profile.name}’ 프로필을 불러왔습니다.`);
  validate();
}

function resetForm() {
  selectedProfileId = "";
  for (const id of fieldIds) setField(id, "");
  setField("connectionMode", "ssh");
  setField("sshUser", "student");
  setField("sshPort", "22");
  setField("socksPort", "1080");
  setField("browserLanguage", "en-US");
  for (const id of ephemeralIds) setField(id, "");
  refreshProfileSelect();
  showProfileMessage("새 프로필을 입력하세요.");
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
  showProfileMessage(`‘${profile.name}’ 프로필을 저장했습니다.`);
  return true;
}

function deleteSelectedProfile() {
  if (connecting || connected || !selectedProfileId) return;
  const profile = profiles.find((item) => item.id === selectedProfileId);
  $("confirmText").textContent = `‘${profile?.name || "선택한"}’ 프로필을 삭제합니다. 현재 입력값도 초기화됩니다.`;
  $("confirmDialog").showModal();
}

function validate(forSave = false) {
  const rhtlc = $("connectionMode").value === "rhtlc";
  $("sshFields").classList.toggle("hidden", rhtlc);
  $("rhtlcFields").classList.toggle("hidden", !rhtlc);
  for (const id of ["privateKeyField", "privateKeyPassphraseField"]) $(id).classList.toggle("hidden", rhtlc);
  $("dashboardHint").textContent = rhtlc
    ? "비워두면 기본 실습 OpenShift 콘솔을 엽니다. RHOAI 등 다른 주소는 직접 입력하세요."
    : "주소를 비워두면 원격 서버에서 자동으로 찾습니다.";
  const ip = $("sshIP").value.trim();
  const user = $("sshUser").value.trim();
  const sshPort = Number($("sshPort").value);
  const socksPort = Number($("socksPort").value);
  const url = $("dashboardURL").value.trim();
  const browserLanguage = $("browserLanguage").value;
  let issue = "";
  const octets = ip.split(".");
  if (!rhtlc && (octets.length !== 4 || octets.some((part) => !/^\d{1,3}$/.test(part) || Number(part) > 255))) {
    issue = "올바른 SSH IPv4 주소를 입력해 주세요.";
  } else if (!rhtlc && !/^[A-Za-z0-9._-]+$/.test(user)) {
    issue = "SSH 사용자 이름을 확인해 주세요.";
  } else if (!rhtlc && (!Number.isInteger(sshPort) || sshPort < 1 || sshPort > 65535)) {
    issue = "SSH 포트는 1~65535 사이여야 합니다.";
  } else if (!Number.isInteger(socksPort) || socksPort < 1024 || socksPort > 65515) {
    issue = "SOCKS 포트는 1024~65515 사이여야 합니다.";
  } else if (!["en-US", "ko-KR"].includes(browserLanguage)) {
    issue = "브라우저 언어를 선택해 주세요.";
  } else if (url) {
    try {
      const parsed = new URL(url);
      if (!["http:", "https:"].includes(parsed.protocol)) throw new Error();
    } catch {
      issue = "대시보드 URL은 http:// 또는 https://로 시작해야 합니다.";
    }
  }
  const profileIssue = issue;
  if (!issue && rhtlc && !connected && !connecting) {
    try {
      const input = fieldValue("rhtlcURL");
      if (!input.startsWith("rhtlc://") || /[\x00-\x1f\x7f]/.test(input)) throw new Error();
      const parsed = new URL(input.replace(/^rhtlc:\/\/(?:https:\/\/|wss:\/\/)?/, "wss://"));
      if (parsed.protocol !== "wss:" || !parsed.hostname || !parsed.searchParams.get("token") || parsed.username || parsed.password || parsed.hash) throw new Error();
    } catch {
      issue = "Copy RHTLC URL로 복사한 rhtlc:// 주소 전체를 붙여넣어 주세요.";
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
    insecureTls: fieldValue("insecureTLS")
  };
}

async function connect() {
  if (connecting || connected) return;
  if (validate()) return;
  if ($("profileName").value.trim()) saveProfile(true);
  disconnectRequested = false;
  setConnectionState("connecting", "연결을 준비하는 중…");
  clearLog();
  appendLog("연결을 준비합니다…");
  try {
    const result = await invoke("start_connection", { config: buildConnectionConfig() });
    $("password").value = "";
    $("privateKeyPassphrase").value = "";
    $("rhtlcURL").value = "";
    if (disconnectRequested) {
      await invoke("stop_connection");
      return;
    }
    connected = true;
    connecting = false;
    setConnectionState("connected", `연결됨 · 127.0.0.1:${result.socksPort}`);
    appendLog(`브라우저에서 ${result.dashboardUrl} 주소를 열었습니다.`);
  } catch (error) {
    connected = false;
    connecting = false;
    if (disconnectRequested) {
      setConnectionState("idle", "연결 안 됨");
      appendLog("연결을 취소했습니다.");
      return;
    }
    setConnectionState("error", "연결 실패");
    const message = String(error || "알 수 없는 오류가 발생했습니다.");
    appendLog(`오류: ${message}`);
    showToast(message);
    $("logPanel").open = true;
  }
}

async function disconnect() {
  disconnectRequested = true;
  $("disconnectButton").disabled = true;
  $("statusText").textContent = "연결을 종료하는 중…";
  try { await invoke("stop_connection"); }
  catch (error) { showToast(String(error)); }
}

async function openIsolatedSession() {
  if (!connected) return;
  const button = $("openSessionButton");
  button.disabled = true;
  try {
    await invoke("open_browser_session");
    showToast("독립 브라우저를 열었습니다. 다른 계정으로 로그인할 수 있습니다.");
  } catch (error) {
    showToast(String(error));
  } finally {
    button.disabled = !connected;
  }
}

async function refreshBrowserDetection(showResultToast = false) {
  const hint = $("browserDetection");
  if (!window.__TAURI_INTERNALS__) {
    hint.textContent = "앱에서 실행하면 설치된 브라우저를 자동으로 확인합니다.";
    hint.className = "field-hint";
    return;
  }

  hint.textContent = "설치된 브라우저를 확인하는 중…";
  hint.className = "field-hint";
  try {
    const browser = await invoke("detect_browser", { requested: fieldValue("browserPath") });
    hint.textContent = `${browser.name} · ${browser.path}`;
    hint.className = "field-hint success";
    if (showResultToast) showToast(`${browser.name}을(를) 사용할 수 있습니다.`);
  } catch (error) {
    hint.textContent = String(error);
    hint.className = "field-hint error";
    if (showResultToast) showToast(String(error));
  }
}

async function refreshRhtlcDetection() {
  const hint = $("rhtlcDetection");
  if (!window.__TAURI_INTERNALS__) {
    hint.textContent = "데스크톱 앱에 RHTLC 연결 기능이 포함되어 있습니다.";
    return;
  }
  try {
    await invoke("detect_rhtlc");
    hint.textContent = "내장 RHTLC 엔진 준비 완료";
    hint.className = "field-hint success";
  } catch (error) {
    hint.textContent = String(error);
    hint.className = "field-hint error";
  }
}

function setConnectionState(state, text) {
  connecting = state === "connecting";
  connected = state === "connected";
  $("statusIcon").className = `status-icon ${state === "idle" ? "" : state}`;
  $("statusText").textContent = text;
  $("connectButton").classList.toggle("hidden", connecting || connected);
  $("openSessionButton").classList.toggle("hidden", !connected);
  $("openSessionButton").disabled = !connected;
  $("disconnectButton").classList.toggle("hidden", !connecting && !connected);
  $("disconnectButton").disabled = false;
  document.querySelector(".app-shell").classList.toggle("busy", connecting || connected);
  if (state === "idle" || state === "error") browserSessionCount = 0;
  validate();
}

function updateTarget() {
  const ip = $("sshIP").value.trim();
  const user = $("sshUser").value.trim() || "student";
  const sessions = connected && browserSessionCount ? ` · 브라우저 ${browserSessionCount}개` : "";
  $("statusTarget").textContent = $("connectionMode").value === "rhtlc"
    ? `RHTLC · VPN 없이 접속${sessions}`
    : ip ? `${user}@${ip}${sessions}` : "SSH IP를 입력하세요";
}

function appendLog(text) {
  const output = $("logOutput");
  if (output.textContent === "아직 실행 기록이 없습니다.") output.textContent = "";
  const time = new Intl.DateTimeFormat("ko-KR", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false }).format(new Date());
  output.textContent += `[${time}] ${text}\n`;
  if (output.textContent.length > 80000) output.textContent = output.textContent.slice(-60000);
  output.scrollTop = output.scrollHeight;
}

function clearLog() { $("logOutput").textContent = "아직 실행 기록이 없습니다."; }

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

async function initialize() {
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
    if (state === "stopped") setConnectionState("idle", "연결 안 됨");
    else if (state === "error") {
      setConnectionState("error", "연결 실패");
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
  showProfileMessage("프로필을 삭제했습니다.");
});
$("connectButton").addEventListener("click", connect);
$("openSessionButton").addEventListener("click", openIsolatedSession);
$("disconnectButton").addEventListener("click", disconnect);
$("detectBrowserButton").addEventListener("click", () => refreshBrowserDetection(true));
$("connectionMode").addEventListener("change", () => {
  validate();
  if (fieldValue("connectionMode") === "rhtlc") void refreshRhtlcDetection();
});
$("clearLogButton").addEventListener("click", clearLog);
for (const id of [...fieldIds, ...ephemeralIds]) $(id).addEventListener("input", validate);
$("browserPath").addEventListener("input", () => {
  $("browserDetection").textContent = "‘경로 확인’을 눌러 사용할 수 있는지 확인하세요.";
  $("browserDetection").className = "field-hint";
});

initialize().catch((error) => showToast(`앱 초기화 오류: ${error}`));
