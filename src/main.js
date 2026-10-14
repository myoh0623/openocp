import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";

const PROFILE_KEY = "remoteOcp.connectionProfiles.v2";
const LAST_PROFILE_KEY = "remoteOcp.lastProfile.v2";
const fieldIds = [
  "profileName", "sshIP", "sshUser", "dashboardURL", "sshPort", "socksPort",
  "browserPath", "browserProfilePath", "privateKeyPath", "keepTemporaryProfile", "insecureTLS"
];
const ephemeralIds = ["password", "privateKeyPassphrase"];
const $ = (id) => document.getElementById(id);
let profiles = loadProfiles();
let selectedProfileId = "";
let connected = false;
let connecting = false;
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
  $("deleteProfileButton").disabled = !selectedProfileId;
}

function applyProfile(profile) {
  selectedProfileId = profile.id;
  setField("profileName", profile.name);
  for (const field of fieldIds.filter((name) => name !== "profileName")) setField(field, profile[field]);
  for (const field of ephemeralIds) setField(field, "");
  localStorage.setItem(LAST_PROFILE_KEY, profile.id);
  refreshProfileSelect();
  showProfileMessage(`‘${profile.name}’ 프로필을 불러왔습니다.`);
  validate();
}

function resetForm() {
  selectedProfileId = "";
  for (const id of fieldIds) setField(id, "");
  setField("sshUser", "student");
  setField("sshPort", "22");
  setField("socksPort", "1080");
  for (const id of ephemeralIds) setField(id, "");
  refreshProfileSelect();
  showProfileMessage("새 프로필을 입력하세요.");
  validate();
  $("sshIP").focus();
}

function saveProfile(silent = false) {
  const issue = validate();
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
  if (!selectedProfileId) return;
  const profile = profiles.find((item) => item.id === selectedProfileId);
  $("confirmText").textContent = `‘${profile?.name || "선택한"}’ 프로필을 삭제합니다. 현재 입력값도 초기화됩니다.`;
  $("confirmDialog").showModal();
}

function validate() {
  const ip = $("sshIP").value.trim();
  const user = $("sshUser").value.trim();
  const sshPort = Number($("sshPort").value);
  const socksPort = Number($("socksPort").value);
  const url = $("dashboardURL").value.trim();
  let issue = "";
  const octets = ip.split(".");
  if (octets.length !== 4 || octets.some((part) => !/^\d{1,3}$/.test(part) || Number(part) > 255)) {
    issue = "올바른 SSH IPv4 주소를 입력해 주세요.";
  } else if (!/^[A-Za-z0-9._-]+$/.test(user)) {
    issue = "SSH 사용자 이름을 확인해 주세요.";
  } else if (!Number.isInteger(sshPort) || sshPort < 1 || sshPort > 65535) {
    issue = "SSH 포트는 1~65535 사이여야 합니다.";
  } else if (!Number.isInteger(socksPort) || socksPort < 1024 || socksPort > 65515) {
    issue = "SOCKS 포트는 1024~65515 사이여야 합니다.";
  } else if (url) {
    try {
      const parsed = new URL(url);
      if (!["http:", "https:"].includes(parsed.protocol)) throw new Error();
    } catch {
      issue = "대시보드 URL은 http:// 또는 https://로 시작해야 합니다.";
    }
  }
  $("validationMessage").textContent = issue;
  $("connectButton").disabled = Boolean(issue) || connecting;
  $("saveProfileButton").disabled = Boolean(issue) || connecting || connected;
  updateTarget();
  return issue;
}

function buildConnectionConfig() {
  return {
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
    keepTemporaryProfile: fieldValue("keepTemporaryProfile"),
    insecureTls: fieldValue("insecureTLS")
  };
}

async function connect() {
  if (validate()) return;
  if ($("profileName").value.trim()) saveProfile(true);
  setConnectionState("connecting", "SSH 서버에 연결하는 중…");
  clearLog();
  appendLog("연결을 준비합니다…");
  try {
    const result = await invoke("start_connection", { config: buildConnectionConfig() });
    $("password").value = "";
    $("privateKeyPassphrase").value = "";
    connected = true;
    connecting = false;
    setConnectionState("connected", `연결됨 · 127.0.0.1:${result.socksPort}`);
    appendLog(`브라우저에서 ${result.dashboardUrl} 주소를 열었습니다.`);
  } catch (error) {
    connected = false;
    connecting = false;
    setConnectionState("error", "연결 실패");
    const message = String(error || "알 수 없는 오류가 발생했습니다.");
    appendLog(`오류: ${message}`);
    showToast(message);
    $("logPanel").open = true;
  }
}

async function disconnect() {
  $("disconnectButton").disabled = true;
  $("statusText").textContent = "연결을 종료하는 중…";
  try { await invoke("stop_connection"); }
  catch (error) { showToast(String(error)); }
}

function setConnectionState(state, text) {
  connecting = state === "connecting";
  connected = state === "connected";
  $("statusIcon").className = `status-icon ${state === "idle" ? "" : state}`;
  $("statusText").textContent = text;
  $("connectButton").classList.toggle("hidden", connecting || connected);
  $("disconnectButton").classList.toggle("hidden", !connecting && !connected);
  $("disconnectButton").disabled = false;
  document.querySelector(".app-shell").classList.toggle("busy", connecting || connected);
  validate();
}

function updateTarget() {
  const ip = $("sshIP").value.trim();
  const user = $("sshUser").value.trim() || "student";
  $("statusTarget").textContent = ip ? `${user}@${ip}` : "SSH IP를 입력하세요";
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
  $("platformBadge").textContent = platform;
  refreshProfileSelect();
  const lastId = localStorage.getItem(LAST_PROFILE_KEY);
  const profile = profiles.find((item) => item.id === lastId) || profiles[0];
  if (profile) applyProfile(profile);
  else resetForm();

  if (!window.__TAURI_INTERNALS__) return;

  await listen("tunnel-log", (event) => appendLog(event.payload));
  await listen("tunnel-status", (event) => {
    const { state, message } = event.payload;
    if (state === "stopped") setConnectionState("idle", "연결 안 됨");
    else if (state === "error") {
      setConnectionState("error", "연결 실패");
      showToast(message);
      $("logPanel").open = true;
    } else if (state === "connecting") setConnectionState("connecting", message);
  });
}

$("profileSelect").addEventListener("change", (event) => {
  const profile = profiles.find((item) => item.id === event.target.value);
  if (profile) applyProfile(profile);
  else resetForm();
});
$("newProfileButton").addEventListener("click", resetForm);
$("saveProfileButton").addEventListener("click", () => saveProfile(false));
$("deleteProfileButton").addEventListener("click", deleteSelectedProfile);
$("confirmDialog").addEventListener("close", () => {
  if ($("confirmDialog").returnValue !== "confirm") return;
  profiles = profiles.filter((item) => item.id !== selectedProfileId);
  persistProfiles();
  localStorage.removeItem(LAST_PROFILE_KEY);
  resetForm();
  showProfileMessage("프로필을 삭제했습니다.");
});
$("connectButton").addEventListener("click", connect);
$("disconnectButton").addEventListener("click", disconnect);
$("clearLogButton").addEventListener("click", clearLog);
for (const id of [...fieldIds, ...ephemeralIds]) $(id).addEventListener("input", validate);

initialize().catch((error) => showToast(`앱 초기화 오류: ${error}`));
