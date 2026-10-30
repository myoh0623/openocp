// Draft profiles may omit the connection address. Supplied values must still
// be valid, and connecting always requires an address for the selected mode.
export function profileIssue(profile, requireAddress = true) {
  const value = (key) => String(profile[key] ?? "").trim();
  const rhtlc = value("connectionMode") === "rhtlc";
  const portValid = (key, min = 1, max = 65535) => {
    const port = Number(value(key));
    return Number.isInteger(port) && port >= min && port <= max;
  };
  const userValid = (key) => /^[A-Za-z0-9._][A-Za-z0-9._-]{0,127}$/.test(value(key));
  if (!rhtlc) {
    const ip = value("sshIP");
    const octets = ip.split(".");
    if ((ip || requireAddress) && (octets.length !== 4 || octets.some((part) => !/^\d{1,3}$/.test(part) || Number(part) > 255))) return "invalidIP";
    if (!userValid("sshUser")) return "invalidUser";
    if (!portValid("sshPort")) return "invalidSSHPort";
  }
  if (!portValid("socksPort", 1024, 65515)) return "invalidSocksPort";
  if (!["en-US", "ko-KR"].includes(value("browserLanguage"))) return "chooseBrowserLanguage";
  if (value("dashboardURL")) {
    try {
      const url = new URL(value("dashboardURL"));
      if (!["http:", "https:"].includes(url.protocol)) throw new Error();
    } catch { return "invalidDashboard"; }
  }
  if (rhtlc && value("remoteHost")) {
    const host = value("remoteHost");
    let valid = /^[A-Za-z0-9][A-Za-z0-9._-]{0,252}$/.test(host);
    if (host.includes(":")) {
      try { valid = new URL(`http://[${host}]/`).hostname.startsWith("["); }
      catch { valid = false; }
    }
    if (!valid) return "invalidRemoteHost";
    if (!userValid("remoteUser")) return "invalidUser";
    if (!portValid("remotePort")) return "invalidSSHPort";
  }
  const folder = value("remoteFolder");
  if (folder && (!folder.startsWith("/") || /[\x00-\x1f\x7f]/.test(folder))) return "invalidRemoteFolder";
  const input = value("rhtlcURL");
  if (rhtlc && (input || requireAddress)) {
    try {
      if (!input.startsWith("rhtlc://") || /[\x00-\x1f\x7f]/.test(input)) throw new Error();
      const url = new URL(input.replace(/^rhtlc:\/\/(?:https:\/\/|wss:\/\/)?/, "wss://"));
      if (url.protocol !== "wss:" || !url.hostname || !url.searchParams.get("token") || url.username || url.password || url.hash) throw new Error();
    } catch { return "invalidRhtlc"; }
  }
  return "";
}
