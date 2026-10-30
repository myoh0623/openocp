import { profileIssue } from "./profile-validation.js";

const fields = ["profileName", "connectionMode", "rhtlcURL", "sshIP", "sshUser", "sshPort",
  "remoteHost", "remoteUser", "remotePort", "remoteFolder", "dashboardURL"];
const titles = ["tutorialConnection", "tutorialWorkstation", "tutorialDashboard", "tutorialReview"];

export function createTutorial({ document, t, onSave, onDismiss }) {
  const $ = (id) => document.getElementById(id);
  const input = (field) => $(`tutorial-${field}`);
  const dialog = $("tutorialDialog");
  let step = 0;
  let original = {};
  let error = "";
  let returnFocus;

  function draft() {
    return { ...original, ...Object.fromEntries(fields.map((field) => [field, input(field).value.trim()])) };
  }

  function render() {
    const profile = draft();
    const rhtlc = profile.connectionMode === "rhtlc";
    for (let index = 0; index < 4; index++) {
      $(`tutorialStep${index}`).hidden = index !== step;
      $(`tutorialProgress${index}`).setAttribute("aria-current", index === step ? "step" : "false");
    }
    $("tutorialStepTitle").textContent = t(titles[step]);
    $("tutorialCounter").textContent = t("tutorialCounter", { step: step + 1 });
    $("tutorialRhtlcFields").hidden = !rhtlc;
    $("tutorialSshFields").hidden = rhtlc;
    $("tutorialRemoteFields").hidden = !rhtlc;
    $("tutorialSshTargetHint").hidden = rhtlc;
    $("tutorialDashboardDefault").textContent = t(rhtlc ? "tutorialDashboardDefaultRhtlc" : "dashboardHintSSH");
    $("tutorialBackButton").disabled = step === 0;
    $("tutorialSkipButton").hidden = step === 3;
    $("tutorialSkipButton").disabled = false;
    $("tutorialNextButton").textContent = t(step === 3 ? "tutorialSave" : "tutorialNext");
    $("tutorialError").textContent = error ? t(error) : "";
    if (step === 3) {
      $("tutorialSummaryName").textContent = profile.profileName || t("tutorialAutoName");
      $("tutorialSummaryConnection").textContent = rhtlc
        ? (profile.rhtlcURL ? t("tutorialRhtlcEntered") : t("tutorialAddressMissing"))
        : (profile.sshIP ? `${profile.sshUser}@${profile.sshIP}:${profile.sshPort}` : t("tutorialAddressMissing"));
      $("tutorialSummaryHost").textContent = rhtlc
        ? (profile.remoteHost ? `${profile.remoteUser}@${profile.remoteHost}:${profile.remotePort}` : t("tutorialHostSkipped"))
        : t("tutorialSameSshTarget");
      $("tutorialSummaryFolder").textContent = profile.remoteFolder || t("tutorialNoFolder");
      $("tutorialSummaryDashboard").textContent = profile.dashboardURL || t(rhtlc ? "tutorialDashboardDefaultRhtlc" : "dashboardHintSSH");
      $("tutorialCompletionHint").textContent = t(profileIssue(profile) ? "tutorialIncomplete" : "tutorialReady");
    }
  }

  function move(next) {
    step = next;
    error = "";
    render();
    $("tutorialBody").scrollTop = 0;
    $("tutorialStepTitle").focus();
  }

  function advance() {
    const profile = draft();
    // Validate the current step independently; later steps can still be edited.
    const candidate = { ...profile };
    if (step === 0) Object.assign(candidate, { remoteHost: "", remoteFolder: "", dashboardURL: "" });
    if (step === 1) candidate.dashboardURL = "";
    error = profileIssue(candidate, false);
    if (error) { render(); $("tutorialError").focus(); return; }
    if (step < 3) move(step + 1);
    else if (onSave(profile)) dialog.close();
  }

  for (const field of fields) input(field).addEventListener("input", () => { error = ""; render(); });
  input("connectionMode").addEventListener("change", () => { error = ""; render(); });
  $("tutorialBackButton").addEventListener("click", () => move(Math.max(0, step - 1)));
  $("tutorialNextButton").addEventListener("click", advance);
  $("tutorialSkipButton").addEventListener("click", () => {
    if (step === 0) input(draft().connectionMode === "rhtlc" ? "rhtlcURL" : "sshIP").value = "";
    if (step === 1) input("remoteHost").value = "";
    if (step === 2) input("dashboardURL").value = "";
    advance();
  });
  $("tutorialLaterButton").addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => {
    // Do not leave a copied lab token in a closed dialog's inputs.
    for (const field of fields) input(field).value = "";
    original = {};
    onDismiss();
    returnFocus?.focus();
  });

  return {
    open(profile, preferRhtlc = false) {
      original = { ...profile };
      returnFocus = document.activeElement;
      for (const field of fields) input(field).value = profile[field] ?? "";
      if (preferRhtlc) input("connectionMode").value = "rhtlc";
      move(0);
      dialog.showModal();
      $("tutorialStepTitle").focus();
    },
    render() { if (dialog.open) render(); }
  };
}
