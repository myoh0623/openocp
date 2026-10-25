# Remote OCP 2.2

Remote OCP opens OpenShift/RHOAI dashboards, SSH terminals, and VS Code Remote - SSH sessions through either an IP/SSH connection or a Red Hat Training Lab Connector (`rhtlc://`) URL. One Rust + Tauri codebase builds the macOS and Windows desktop apps.

## App language

Use **App language** in the sidebar to switch between **English** and **한국어**. English is the default. The choice is remembered for the next launch and applies to the interface, validation, and new connection messages. It is independent of the browser language in each connection profile.

## Features

- Choose IP/SSH or RHTLC URL connection mode.
- Connect to training labs without a VPN or a separately installed RHTLC client.
- Launch Chrome, Chromium, or Edge with the correct SOCKS5 proxy and an isolated browser profile.
- Open multiple isolated browser sessions on one connection, for example admin and developer logins.
- Save connection profiles and browser language preferences (English by default).
- Keep passwords, private-key passphrases, and token-bearing RHTLC URLs out of saved profiles.
- Detect OpenShift/RHOAI routes through SSH when the dashboard URL is blank.
- Open the native Terminal or VS Code against the same lab connection.
- Keep the connection alive when browser windows close; Disconnect or app exit stops the tunnel and forwarded SSH sessions.

## Connect with an RHTLC URL

1. Start your lab in the Red Hat training portal and select **Copy RHTLC URL**.
2. In Remote OCP, choose **RHTLC URL** connection mode and paste the complete address.
3. Enter the dashboard URL, or leave it blank to open the default training console: `https://console-openshift-console.apps.ocp4.example.com/`. Enter the URL explicitly for RHOAI or a different cluster.
4. For Terminal or VS Code, enter the lab **SSH host**, user, and port under **Terminal & VS Code**. Use the host provided for your lab (for example, `workstation.lab.example.com`); leave it blank for dashboard access only.
5. Select **Connect**. Remote OCP verifies SSH access when an SSH host is supplied, otherwise it checks dashboard connectivity.
6. Choose **Dashboard**, **Terminal**, or **VS Code**. Each Dashboard click opens an independent browser session. **Disconnect** closes owned browsers, the tunnel, and forwarded SSH connections.

The desktop installer includes the small `rhtlc-wstunnel` transport engine. The official Python CLI/GUI, Firefox, and a VPN are not required. Dashboard browsing requires Chrome, Chromium, or Edge. Terminal and VS Code work without a browser. No system proxy settings, existing browser profiles, or official RHTLC/SSH configuration files are changed. Leave the browser profile folder blank to create a temporary profile automatically.

RHTLC URLs are used only for the current connection. Paste a fresh URL when reconnecting. Following the official RHTLC 6.0.2 client's address handling, Remote OCP converts the lab endpoint to `wss://` and removes the portal query, including its token, before starting the transport. Tokens are never passed in process arguments, written to configuration, or included in logs. The public gateway's TLS certificate is verified; private lab hostnames are resolved through the SOCKS tunnel.

This integration covers dashboard browsing and SSH access through external tools. VPN, RDP, and SFTP interfaces are outside its scope. See the [official RHTLC documentation](https://redhattraining.github.io/RHTLC_Docs/) for those workflows.

## Connect with IP/SSH

1. Choose **IP/SSH** and enter the SSH IP address and user.
2. Enter a password, or use an SSH private key. A private-key passphrase can be supplied in advanced settings.
3. Enter a dashboard URL, or leave it blank for remote route discovery.
4. Select **Connect**, then choose **Dashboard**, **Terminal**, or **VS Code**.

Each additional browser gets a separate temporary profile and independent cookies. Browser language preferences apply to new sessions. Advanced settings let you inspect the detected browser or select its executable manually. SSH mode still requires network access to the specified IP, including a VPN if the environment requires one.

## Terminal and VS Code

Connect first, then choose a tool. In IP/SSH mode, tools use the configured SSH IP, port, and user. In RHTLC mode, tools reach the configured lab host through the bundled SOCKS transport, with hostname resolution inside the lab. A local SSH relay listens only on `127.0.0.1` and is shared by the opened tools.

- **Terminal** opens macOS Terminal or Windows Terminal. Windows falls back to a separate PowerShell window when Windows Terminal is unavailable. Windows requires the **OpenSSH Client** optional feature.
- **VS Code** requires the installed stable VS Code application and Microsoft's **Remote - SSH** extension. It opens a dedicated Remote OCP VS Code window, sharing installed extensions while keeping user settings separate. Advanced settings can specify the VS Code app or executable; use `Code.exe`, not `code.cmd`, on Windows.
- **VS Code remote folder** is optional. Enter an absolute Linux path such as `/home/student/project`; leave it blank to connect without opening a folder. The target must meet [VS Code Remote - SSH requirements](https://code.visualstudio.com/docs/remote/ssh), including permission and network access for VS Code Server installation.
- Complete SSH authentication in the opened application. Passwords and passphrases supplied to Remote OCP are not transferred to external tools. Set a private-key path in advanced settings if needed; encrypted keys are unlocked in the external application.

Remote OCP creates private, session-specific SSH configuration files and uses a dedicated known-hosts file in its app data directory. OpenSSH asks before accepting an unknown host key and rejects changed keys. Host identities stay stable across local port changes and are separated by lab gateway and target. Existing `~/.ssh/config` files are not modified or loaded by the generated configuration. Dedicated VS Code profiles are stored under `~/.remote-ocp/code-<app-id-hash>` (a short path compatible with macOS socket limits); keep its `User/settings.json` valid JSON.

Closing a browser, terminal, or editor does **not** end the connection. Select **Disconnect**, or exit Remote OCP, when finished. Disconnect closes the app-owned relay and active SSH streams and removes session launch files. Terminal and VS Code windows remain open so unsaved work is not discarded. Reconnect using the tool buttons in Remote OCP; old window connection targets belong to the previous session.

## Download builds

In GitHub, open **Actions → Build macOS and Windows** and select a successful run:

- `Remote-OCP-macOS`: Universal macOS app and DMG (Apple Silicon and Intel).
- `Remote-OCP-Windows`: Windows x64 EXE and MSI installers.

Both platforms are built for pull requests and pushes to `main`. Windows may prompt about the bundled tunnel executable under local firewall/security policies.

## Build locally

Requirements: Node.js 22+, Rust stable, and the platform's Tauri prerequisites (Xcode Command Line Tools on macOS; Microsoft C++ Build Tools and WebView2 on Windows).

```bash
npm ci
npm run tauri build
```

The Tauri dev/build hooks run `npm run prepare:rhtlc` automatically. This downloads the pinned **rhtlc-wstunnel 6.0.1** release for the target, verifies a hardcoded SHA-256 checksum, and stages the transport as a Tauri sidecar. Universal macOS builds combine the verified ARM64 and x64 binaries using `lipo`. Downloads happen at build time only; installed apps use the bundled executable without downloading or searching for another client.

```bash
# macOS Universal
npm run tauri build -- --target universal-apple-darwin

# Prepare the host engine before invoking Cargo directly
npm run prepare:rhtlc
cargo test --manifest-path src-tauri/Cargo.toml
```

If project-local Rust tools are available in `.tools`, `./build-desktop.sh` uses them. Bundles are written under `src-tauri/target/<target>/release/bundle`, or `src-tauri/target/release/bundle` for a default-target build.

The transport adds approximately 9.6 MiB on macOS ARM64 and 9.9 MiB on Windows x64 before installer compression. Universal macOS includes both architectures. A single owned transport process is shared by all browser sessions and uses two worker threads. Actual memory use depends on traffic and browser sessions; no fixed RAM guarantee is made.

Source and redistribution notices are in [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt), which is included in the app bundle. Binary hashes and download URLs are pinned in [scripts/prepare-rhtlc.mjs](scripts/prepare-rhtlc.mjs). Remote OCP is an independent project.

## Project layout

- `src/`: Shared UI and connection profiles.
- `src-tauri/src/`: Rust SSH, SOCKS5, route discovery, and browser lifecycle management.
- `src-tauri/src/remote_tools.rs`: Loopback SSH forwarding, private OpenSSH configuration, and native tool launchers.
- `src-tauri/src/rhtlc.rs`: Bundled transport lifecycle, URL handling, and SOCKS connectivity checks.
- `scripts/prepare-rhtlc.mjs`: Verified transport preparation for macOS and Windows builds.
- `.github/workflows/`: Desktop builds and connection tests.

Training dashboard addresses under `*.example.com` automatically allow browser certificate errors. Other dashboard addresses require the explicit advanced setting. This browser setting does not disable TLS verification for the public RHTLC gateway.

## License

Remote OCP is licensed under the [MIT License](LICENSE). Third-party dependencies retain their respective licenses. The bundled `rhtlc-wstunnel` engine is distributed under BSD-3-Clause; its original copyright, license conditions, and disclaimer are preserved in [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt). Both files are included in the app bundle.
