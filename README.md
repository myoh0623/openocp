# Remote OCP

Remote OCP is a macOS and Windows desktop app that opens OpenShift and RHOAI dashboards through an SSH SOCKS tunnel. One **Rust + Tauri** codebase builds both desktop apps.

## Features

- Connect using an SSH IP address, username, port, and optional dashboard URL.
- Discover the OpenShift or RHOAI route on the remote server when the dashboard URL is blank.
- Save connection profiles for later use without storing passwords or private-key passphrases.
- Use the built-in Rust SSH client and SOCKS5 proxy.
- Detect Google Chrome, Chromium, or Microsoft Edge automatically and show the selected executable path.
- Choose English or Korean for the browser in each profile, with English as the default.
- Open multiple isolated browser sessions on the same tunnel with separate login information.
- Close the tunnel when the last owned browser process exits.
- Use the same interface and connection features on macOS and Windows.

## Connect to a dashboard

1. Launch Remote OCP for your operating system.
2. Enter the SSH IP address and username.
3. Enter the SSH password for password authentication, or leave it blank to use a private key. Enter a private-key passphrase in advanced settings if needed.
4. Enter the dashboard URL, or leave it blank to discover the route automatically.
5. Select **Open dashboard**.
6. Select **+ Isolated browser** to sign in with another account independently.
7. Close all browsers opened by Remote OCP, or select **Disconnect**, to stop the tunnel.

Connection profiles do not include SSH passwords or private-key passphrases.

Each isolated browser uses a separate temporary profile, so cookies and login sessions are not shared. For example, one window can use an `admin` account while another uses a `developer` account. The profile's browser language also applies to additional browser sessions.

Remote OCP checks common installation locations and the system path for a compatible browser. In advanced settings, select **Check path** to verify the selected browser and executable. If automatic detection fails, enter the executable path manually.

The SSH server must be reachable from your computer. Depending on the training environment, this may require a VPN.

## Download builds

Open **Actions → Build macOS and Windows** in this repository and select a successful run. Its artifacts include:

- `Remote-OCP-macOS`: Universal macOS app and DMG for Apple Silicon and Intel.
- `Remote-OCP-Windows`: Windows x64 EXE and MSI installers.

The workflow builds both platforms for pull requests and pushes to `main`.

## Upcoming version 2.2

[Pull request #3](https://github.com/myoh0623/openocp/pull/3) adds an English/Korean app interface and RHTLC URL connections with a bundled transport. Those changes are available in the PR branch and its successful build artifacts until the PR is merged. App language is separate from the browser language supported by the current main branch.

## Build locally

Requirements:

- Node.js 22 or later.
- Rust stable.
- Xcode Command Line Tools to build on macOS.
- Microsoft C++ Build Tools and WebView2 to build on Windows.

```bash
npm ci
npm run tauri build
```

If project-local Rust tools are available in `.tools`, you can also use:

```bash
./build-desktop.sh
```

Default-target bundles are written to `src-tauri/target/release/bundle`. Builds with an explicit target use `src-tauri/target/<target>/release/bundle`.

## Project layout

- `src/`: Shared interface and connection profile management.
- `src-tauri/src/`: Rust SSH, SOCKS5, route discovery, and browser lifecycle management.
- `src-tauri/icons/`: macOS and Windows app icons.
- `.github/workflows/`: Automated macOS and Windows builds.

Training dashboard addresses under `*.example.com` automatically allow browser certificate errors. Other dashboard addresses require the explicit advanced setting.

## License

Remote OCP is licensed under the [MIT License](LICENSE). Third-party dependencies retain their respective licenses.
