//! App-owned loopback SSH forwarding and external tool launchers.
//! Credentials are entered in OpenSSH/VS Code, never written to launch files.
use super::*;
use sha2::{Digest, Sha256};
use std::process::Stdio;

#[derive(Clone, Debug)]
pub(crate) struct Route {
    pub host: String,
    pub port: u16,
    pub socks_port: Option<u16>,
    pub scope: String,
}

impl Route {
    async fn connect(&self) -> Result<TcpStream> {
        match self.socks_port {
            Some(port) => rhtlc::connect_socks(port, &self.host, self.port).await,
            None => Ok(timeout(
                Duration::from_secs(15),
                TcpStream::connect((self.host.as_str(), self.port)),
            )
            .await??),
        }
    }

    fn host_key_alias(&self) -> String {
        let identity = format!("{}\0{}\0{}", self.scope, self.host, self.port);
        format!("remote-ocp-{:x}", Sha256::digest(identity.as_bytes()))
    }
}

pub(crate) fn valid_target(host: &str) -> bool {
    host.parse::<IpAddr>().is_ok()
        || (!host.is_empty()
            && host.len() <= 253
            && host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c))
            && host.as_bytes()[0].is_ascii_alphanumeric())
}

pub(crate) fn validate_user(user: &str) -> Result<()> {
    if user.is_empty()
        || user.len() > 128
        || !user
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        || user.starts_with('-')
    {
        tr_bail!(
            "Enter a valid SSH username for developer tools.",
            "개발 도구에 사용할 올바른 SSH 사용자 이름을 입력하세요."
        );
    }
    Ok(())
}

pub(crate) async fn probe_ssh(route: &Route) -> Result<()> {
    timeout(Duration::from_secs(15), async {
        let mut socket = route.connect().await?;
        // RFC 4253 permits informational lines before the SSH identification.
        let mut line = Vec::new();
        for _ in 0..8192 {
            let byte = socket.read_u8().await?;
            line.push(byte);
            if byte == b'\n' {
                if line.starts_with(b"SSH-2.0-") || line.starts_with(b"SSH-1.99-") {
                    return Ok(());
                }
                line.clear();
            }
        }
        tr_bail!(
            "The target did not identify itself as an SSH server.",
            "대상 서버에서 SSH 응답을 확인하지 못했습니다."
        );
    })
    .await
    .context(tr!(
        "SSH target verification timed out.",
        "SSH 대상 확인 시간이 초과되었습니다."
    ))?
}

pub(crate) struct Session {
    alias: String,
    directory: PathBuf,
    config: PathBuf,
    ssh: PathBuf,
    bridge: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(bridge) = &self.bridge {
            bridge.abort();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl Session {
    pub async fn shutdown(mut self) {
        if let Some(bridge) = self.bridge.take() {
            bridge.abort();
            let _ = bridge.await;
        }
        // Drop removes launch files after the listener and its clients close.
    }

    pub async fn prepare(
        root: &Path,
        route: Route,
        user: &str,
        key: &str,
        stop: watch::Receiver<bool>,
    ) -> Result<Self> {
        validate_user(user)?;
        if !valid_target(&route.host) || route.port == 0 {
            tr_bail!(
                "Enter the SSH target host and port.",
                "SSH 대상 호스트와 포트를 입력하세요."
            );
        }
        let ssh = find_ssh()?;
        probe_ssh(&route).await.context(tr!(
            "Cannot reach the SSH target. Check its address and SSH service.",
            "SSH 대상에 연결할 수 없습니다. 주소와 SSH 서비스 상태를 확인하세요."
        ))?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        private_directory(root)?;
        let alias = format!("remote-ocp-{}", Uuid::new_v4().simple());
        let directory = root.join(&alias);
        let config = directory.join("ssh_config");
        let contents = ssh_config(
            &alias,
            port,
            user,
            key,
            &root.join("known_hosts"),
            &route.host_key_alias(),
        )?;
        private_directory(&directory)?;
        if let Err(error) = private_file(&config, contents.as_bytes(), false) {
            let _ = std::fs::remove_dir_all(&directory);
            return Err(error);
        }
        let bridge = tokio::spawn(forward(listener, route, stop));
        Ok(Self {
            alias,
            directory,
            config,
            ssh,
            bridge: Some(bridge),
        })
    }

    pub async fn open_terminal(&self, app: &AppHandle, stop: watch::Receiver<bool>) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let script = self.directory.join("Remote OCP.command");
            let text = format!(
                "#!/bin/sh\nexec {} -F {} {}\n",
                shell_quote(&self.ssh.to_string_lossy()),
                shell_quote(&self.config.to_string_lossy()),
                shell_quote(&self.alias)
            );
            private_file(&script, text.as_bytes(), true)?;
            return launch_detached(
                Command::new("/usr/bin/open")
                    .args(["-a", "Terminal"])
                    .arg(&script),
                "Terminal",
                launch_error_reporter(app, stop),
            )
            .await;
        }
        #[cfg(windows)]
        {
            if let Some(wt) = find_in_path("wt.exe") {
                launch_detached(
                    Command::new(wt)
                        .args(["new-tab", "--title", "Remote OCP"])
                        .arg(&self.ssh)
                        .arg("-F")
                        .arg(&self.config)
                        .arg(&self.alias),
                    "Windows Terminal",
                    launch_error_reporter(app, stop),
                )
                .await?;
            } else {
                let text = format!(
                    "& {} -F {} {}",
                    powershell_quote(&self.ssh.to_string_lossy()),
                    powershell_quote(&self.config.to_string_lossy()),
                    powershell_quote(&self.alias)
                );
                launch_detached(
                    &mut windows_console_command(&text)?,
                    "PowerShell",
                    launch_error_reporter(app, stop),
                )
                .await?;
            }
            return Ok(());
        }
        #[allow(unreachable_code)]
        Err(anyhow!(tr!(
            "External terminals are supported on macOS and Windows.",
            "외부 터미널은 macOS와 Windows에서 지원합니다."
        )))
    }

    pub async fn open_vscode(
        &self,
        root: &Path,
        requested: &str,
        folder: &str,
        app: &AppHandle,
        stop: watch::Receiver<bool>,
    ) -> Result<()> {
        let code = supported_vscode(requested)?;
        let extensions = extensions_directory()?;
        require_remote_ssh(&extensions)?;
        // macOS limits Unix socket paths to 103 bytes. A long app-data path
        // makes VS Code exit before creating a window; keep its profile short.
        let base = home_directory()
            .context("Home directory is unavailable")?
            .join(".remote-ocp");
        private_directory(&base)?;
        let profile = format!("{:x}", Sha256::digest(root.to_string_lossy().as_bytes()));
        let data = base.join(format!("code-{}", &profile[..12]));
        #[cfg(unix)]
        if data.join("1.100-main.sock").as_os_str().len() > 103 {
            tr_bail!(
                "The home directory path is too long for VS Code's local socket.",
                "홈 디렉터리 경로가 너무 길어 VS Code의 로컬 소켓을 만들 수 없습니다."
            );
        }
        private_directory(&data)?;
        let user = data.join("User");
        private_directory(&user)?;
        let settings_path = user.join("settings.json");
        update_vscode_settings(&settings_path, &self.config, &self.ssh)?;
        let mut command = Command::new(code);
        command
            .args(["--new-window", "--user-data-dir"])
            .arg(data)
            .arg("--extensions-dir")
            .arg(extensions);
        if folder.is_empty() {
            command
                .arg("--remote")
                .arg(format!("ssh-remote+{}", self.alias));
        } else {
            command
                .arg("--folder-uri")
                .arg(folder_uri(&self.alias, folder)?);
        }
        launch_detached(&mut command, "VS Code", launch_error_reporter(app, stop)).await
    }
}

fn launch_error_reporter(
    app: &AppHandle,
    stop: watch::Receiver<bool>,
) -> impl FnOnce(String) + Send + 'static {
    let app = app.clone();
    move |message| {
        if !*stop.borrow() {
            let _ = app.emit("tool-error", message);
        }
    }
}

fn launch_status(status: std::process::ExitStatus, name: &str) -> Result<()> {
    if !status.success() {
        tr_bail!(
            "{} exited with an error ({}). Check the application installation and its logs, then try again.",
            "{}이(가) 오류로 종료되었습니다({}). 프로그램 설치 상태와 해당 실행 기록을 확인한 뒤 다시 시도하세요.",
            name,
            status
        );
    }
    Ok(())
}

#[cfg(windows)]
fn windows_console_command(script: &str) -> Result<Command> {
    use base64::Engine;

    let system32 = env::var_os("SystemRoot")
        .map(PathBuf::from)
        .context("SystemRoot is unavailable")?
        .join("System32");
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    let mut command = Command::new(system32.join("cmd.exe"));
    // CREATE_NEW_CONSOLE alone does not replace explicitly inherited NUL/pipe
    // handles. Reopen the new console's buffers before starting PowerShell so
    // both it and native SSH receive real interactive stdin/stdout/stderr.
    // Only Base64 and a quoted, app-supplied system executable enter cmd syntax;
    // paths/user arguments remain inside the encoded PowerShell command.
    command
        .args(["/D", "/V:OFF", "/S", "/C"])
        .raw_arg(format!(
            "\"\"%REMOTE_OCP_POWERSHELL%\" -NoLogo -NoProfile -NoExit -EncodedCommand {encoded} <CONIN$ >CONOUT$ 2>&1\""
        ))
        .env(
            "REMOTE_OCP_POWERSHELL",
            system32.join("WindowsPowerShell/v1.0/powershell.exe"),
        )
        .creation_flags(0x00000010); // CREATE_NEW_CONSOLE
    Ok(command)
}

async fn launch_detached(
    command: &mut Command,
    name: &str,
    on_error: impl FnOnce(String) + Send + 'static,
) -> Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context(tr!(
            "Could not launch the external application. Check its executable and permissions.",
            "외부 프로그램을 실행하지 못했습니다. 실행 파일과 권한을 확인하세요."
        ))?;
    // Detect immediate failure, but allow long-lived editors and successful
    // handoff to existing windows. Never kill the user's editor on drop.
    match timeout(Duration::from_secs(2), child.wait()).await {
        Ok(status) => launch_status(status?, name),
        Err(_) => {
            let name = name.to_owned();
            tokio::spawn(async move {
                let result = child
                    .wait()
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|status| launch_status(status, &name));
                if let Err(error) = result {
                    on_error(error.to_string());
                }
            });
            Ok(())
        }
    }
}

fn update_vscode_settings(path: &Path, config: &Path, ssh: &Path) -> Result<()> {
    use jsonc_parser::{ParseOptions, cst::CstRootNode};
    let text = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        "{}\n".into()
    };
    // Match VS Code JSONC, without accepting malformed JSON5 syntax.
    let options = ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    };
    let root = CstRootNode::parse(&text, &options).context(tr!(
        "Cannot read the dedicated VS Code settings file. Fix its JSON syntax and try again.",
        "전용 VS Code 설정 파일을 읽지 못했습니다. JSON 문법을 수정한 뒤 다시 시도하세요."
    ))?;
    let object = root.object_value_or_create().context(tr!(
        "VS Code settings must contain a JSON object.",
        "VS Code 설정은 JSON 객체여야 합니다."
    ))?;
    for (key, value) in [
        (
            "remote.SSH.configFile",
            jsonc_parser::json!(config.to_string_lossy().as_ref()),
        ),
        (
            "remote.SSH.path",
            jsonc_parser::json!(ssh.to_string_lossy().as_ref()),
        ),
        ("remote.SSH.showLoginTerminal", jsonc_parser::json!(true)),
    ] {
        if let Some(property) = object.get(key) {
            property.set_value(value);
        } else {
            object.append(key, value);
        }
    }
    private_file(path, root.to_string().as_bytes(), false)
}

async fn forward(listener: TcpListener, route: Route, mut stop: watch::Receiver<bool>) {
    let mut clients = JoinSet::new();
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            _ = stop.changed() => break,
            _ = clients.join_next(), if !clients.is_empty() => {},
            accepted = listener.accept() => {
                let Ok((mut local, _)) = accepted else { break; };
                let route = route.clone();
                let mut stop = stop.clone();
                clients.spawn(async move {
                    if *stop.borrow() { return; }
                    tokio::select! {
                        _ = stop.changed() => {},
                        _ = async {
                            let mut remote = route.connect().await?;
                            tokio::io::copy_bidirectional(&mut local, &mut remote).await?;
                            Ok::<(), anyhow::Error>(())
                        } => {},
                    }
                });
            }
        }
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
}

fn private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn private_file(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if executable { 0o700 } else { 0o600 });
    }
    #[cfg(not(unix))]
    let _ = executable;
    options.open(path)?.write_all(bytes)?;
    Ok(())
}

fn config_quote(value: &str) -> Result<String> {
    if value.chars().any(char::is_control) {
        tr_bail!(
            "SSH paths must not contain control characters.",
            "SSH 경로에는 제어 문자를 사용할 수 없습니다."
        );
    }
    #[cfg(windows)]
    let value = value.replace('\\', "/");
    #[cfg(not(windows))]
    let value = value.replace('\\', "\\\\");
    Ok(format!(
        "\"{}\"",
        value.replace('%', "%%").replace('"', "\\\"")
    ))
}

fn ssh_config(
    alias: &str,
    port: u16,
    user: &str,
    key: &str,
    known_hosts: &Path,
    host_key: &str,
) -> Result<String> {
    validate_user(user)?;
    let mut text = format!(
        "Host {alias}\n  HostName 127.0.0.1\n  Port {port}\n  User {user}\n  HostKeyAlias {host_key}\n  UserKnownHostsFile {}\n  StrictHostKeyChecking ask\n  CheckHostIP no\n  ForwardAgent no\n  ControlMaster no\n  ControlPath none\n  ServerAliveInterval 30\n  ServerAliveCountMax 3\n  ConnectTimeout 15\n",
        config_quote(&known_hosts.to_string_lossy())?
    );
    if !key.is_empty() {
        let key = expand_home(key);
        if !key.is_file() {
            tr_bail!(
                "The selected SSH private key was not found.",
                "선택한 SSH 개인 키를 찾을 수 없습니다."
            );
        }
        text.push_str(&format!(
            "  IdentityFile {}\n  IdentitiesOnly yes\n",
            config_quote(&key.to_string_lossy())?
        ));
    }
    Ok(text)
}

#[cfg(any(target_os = "macos", test))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
#[cfg(any(windows, test))]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub(crate) fn validate_folder(folder: &str) -> Result<()> {
    if !folder.starts_with('/') || folder.chars().any(char::is_control) {
        tr_bail!(
            "Enter an absolute remote folder, such as /home/student, or leave it blank.",
            "/home/student 같은 원격 절대 경로를 입력하거나 비워두세요."
        );
    }
    Ok(())
}

fn folder_uri(alias: &str, folder: &str) -> Result<String> {
    validate_folder(folder)?;
    let mut url = Url::parse(&format!("vscode-remote://ssh-remote+{alias}"))?;
    url.set_path(&folder.replace('%', "%25"));
    Ok(url.to_string())
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    })
}

fn find_ssh() -> Result<PathBuf> {
    #[cfg(windows)]
    let ssh = env::var_os("SystemRoot")
        .map(|p| PathBuf::from(p).join("System32/OpenSSH/ssh.exe"))
        .filter(|p| p.is_file())
        .or_else(|| find_in_path("ssh.exe"));
    #[cfg(not(windows))]
    let ssh = Some(PathBuf::from("/usr/bin/ssh"))
        .filter(|p| p.is_file())
        .or_else(|| find_in_path("ssh"));
    ssh.context(tr!("OpenSSH was not found. Install the OpenSSH Client to use Terminal or VS Code.", "OpenSSH를 찾지 못했습니다. 터미널이나 VS Code를 사용하려면 OpenSSH 클라이언트를 설치하세요."))
}

fn find_vscode(requested: &str) -> Result<PathBuf> {
    if !requested.is_empty() {
        let mut path = expand_home(requested);
        if path.extension().is_some_and(|ext| ext == "app") {
            path = path.join("Contents/Resources/app/bin/code");
        }
        if path.is_file()
            && !path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat")
            })
        {
            return Ok(path);
        }
        tr_bail!(
            "Choose the VS Code app, code executable, or Code.exe (not a .cmd file).",
            "VS Code 앱, code 실행 파일 또는 Code.exe를 선택하세요(.cmd 제외)."
        );
    }
    let mut candidates = Vec::new();
    if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from(
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
        ));
        if let Some(home) = home_directory() {
            candidates.push(
                home.join("Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"),
            );
        }
    }
    if cfg!(windows) {
        for variable in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(base) = env::var_os(variable) {
                let relative = if variable == "LOCALAPPDATA" {
                    "Programs/Microsoft VS Code/Code.exe"
                } else {
                    "Microsoft VS Code/Code.exe"
                };
                candidates.push(PathBuf::from(base).join(relative));
            }
        }
    }
    if let Some(path) = find_in_path(if cfg!(windows) { "Code.exe" } else { "code" }) {
        candidates.push(path);
    }
    candidates.into_iter().find(|p| p.is_file()).context(tr!(
        "VS Code was not found. Install it or set its executable in advanced settings.",
        "VS Code를 찾지 못했습니다. 설치하거나 고급 설정에서 실행 파일을 지정하세요."
    ))
}

fn portable_data(code: &Path) -> Option<PathBuf> {
    let code = code.canonicalize().unwrap_or_else(|_| code.to_path_buf());
    // Check both layouts so explicit executable paths behave consistently.
    if let Some(bundle) = code
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
    {
        return bundle
            .parent()
            .map(|p| p.join("code-portable-data"))
            .filter(|p| p.is_dir());
    }
    code.parent().map(|p| p.join("data")).filter(|p| p.is_dir())
}

fn supported_vscode(requested: &str) -> Result<PathBuf> {
    let code = find_vscode(requested)?;
    if portable_data(&code).is_some()
        || env::var_os("VSCODE_PORTABLE").is_some_and(|v| !v.is_empty())
    {
        tr_bail!(
            "Portable VS Code overrides the separate connection profile and is not supported. Select a standard VS Code installation in advanced settings.",
            "포터블 VS Code는 전용 연결 프로필을 무시하므로 지원하지 않습니다. 설치형 VS Code를 설치하고 고급 설정에서 선택하세요."
        );
    }
    Ok(code)
}

fn extensions_directory() -> Result<PathBuf> {
    if let Some(path) = env::var_os("VSCODE_EXTENSIONS").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    Ok(home_directory()
        .context("Home directory is unavailable")?
        .join(".vscode/extensions"))
}

fn require_remote_ssh(extensions: &Path) -> Result<()> {
    if !has_remote_ssh(extensions) {
        tr_bail!(
            "Install Microsoft's Remote - SSH extension in VS Code, then select Check again.",
            "VS Code에서 Microsoft의 Remote - SSH 확장을 설치한 뒤 ‘다시 확인’을 누르세요."
        );
    }
    Ok(())
}

#[derive(Serialize)]
pub(crate) struct DependencyStatus {
    available: bool,
    message: String,
}

impl DependencyStatus {
    fn from_result(result: Result<()>) -> Self {
        match result {
            Ok(()) => Self {
                available: true,
                message: tr!("Ready", "준비 완료").into(),
            },
            Err(error) => Self {
                available: false,
                message: error.to_string(),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolAvailability {
    ssh: DependencyStatus,
    vscode: DependencyStatus,
    remote_ssh: DependencyStatus,
}

pub(crate) fn detect_tools(requested: &str) -> ToolAvailability {
    ToolAvailability {
        ssh: DependencyStatus::from_result(find_ssh().map(|_| ())),
        vscode: DependencyStatus::from_result(supported_vscode(requested).map(|_| ())),
        remote_ssh: DependencyStatus::from_result(
            extensions_directory().and_then(|p| require_remote_ssh(&p)),
        ),
    }
}

pub(crate) fn help_url(topic: &str) -> Result<&'static str> {
    match topic {
        "github" => Ok("https://github.com/myoh0623/openocp"),
        "ssh" => Ok(
            "https://learn.microsoft.com/en-us/windows-server/administration/openssh/openssh-install-firstuse",
        ),
        "vscode" => Ok("https://code.visualstudio.com/download"),
        "remote-ssh" => {
            Ok("https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-ssh")
        }
        _ => tr_bail!("Unknown installation guide.", "알 수 없는 설치 안내입니다."),
    }
}

pub(crate) async fn open_help(topic: &str) -> Result<()> {
    let url = help_url(topic)?;
    #[cfg(target_os = "macos")]
    let mut command = Command::new("/usr/bin/open");
    #[cfg(windows)]
    let mut command = {
        let root = env::var_os("SystemRoot").context("SystemRoot is unavailable")?;
        let mut command = Command::new(PathBuf::from(root).join("System32/rundll32.exe"));
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = Command::new("xdg-open");
    let status = timeout(Duration::from_secs(10), command.arg(url).status()).await??;
    launch_status(status, tr!("Browser", "브라우저"))
}

fn has_remote_ssh(extensions: &Path) -> bool {
    let obsolete: serde_json::Value = std::fs::read(extensions.join(".obsolete"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    std::fs::read_dir(extensions).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let candidate = entry
                .file_name()
                .to_string_lossy()
                .starts_with("ms-vscode-remote.remote-ssh-");
            candidate
                && obsolete[entry.file_name().to_string_lossy().as_ref()] != true
                && std::fs::read(entry.path().join("package.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .is_some_and(|package| {
                        package["name"] == "remote-ssh"
                            && package["publisher"] == "ms-vscode-remote"
                    })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(host: &str, port: u16) -> Route {
        Route {
            host: host.into(),
            port,
            socks_port: None,
            scope: "lab-a".into(),
        }
    }

    #[test]
    fn vscode_settings_preserve_jsonc_and_reject_invalid_content_without_overwriting() {
        let root = env::temp_dir().join(format!("remote-ocp-jsonc-{}", Uuid::new_v4()));
        private_directory(&root).unwrap();
        let path = root.join("settings.json");
        let original = r#"{
  // My editor preferences
  "editor.fontSize": 16,
  "homepage": "https://example.com/", /* Keep this comment */
  "remote.SSH.configFile": "old-session",
}"#;
        private_file(&path, original.as_bytes(), false).unwrap();
        let config = root.join("new session");
        let ssh = root.join("ssh");
        update_vscode_settings(&path, &config, &ssh).unwrap();
        let output = std::fs::read_to_string(&path).unwrap();
        assert!(output.contains("// My editor preferences"));
        assert!(output.contains("/* Keep this comment */"));
        assert!(output.contains("\"editor.fontSize\": 16,"));
        assert!(output.contains("https://example.com/"));
        assert!(output.contains(&serde_json::to_string(&config).unwrap()));
        assert!(!output.contains("old-session"));
        update_vscode_settings(&path, &config, &ssh).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), output);
        for invalid in ["[]", "null", "{broken", r#"{"a": 1 "b": 2}"#] {
            private_file(&path, invalid.as_bytes(), false).unwrap();
            assert!(update_vscode_settings(&path, &config, &ssh).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn portable_installations_are_identified_before_any_settings_are_written() {
        let root = env::temp_dir().join(format!("remote-ocp-portable-{}", Uuid::new_v4()));
        private_directory(&root.join("data")).unwrap();
        let code = root.join("Code.exe");
        private_file(&code, b"test", false).unwrap();
        assert!(portable_data(&code).is_some());
        assert!(supported_vscode(code.to_str().unwrap()).is_err());
        assert!(!root.join("data/User/settings.json").exists());
        let bundle = root.join("Visual Studio Code.app/Contents/Resources/app/bin");
        private_directory(&bundle).unwrap();
        private_file(&bundle.join("code"), b"test", false).unwrap();
        assert!(portable_data(&bundle.join("code")).is_none());
        private_directory(&root.join("code-portable-data")).unwrap();
        assert!(portable_data(&bundle.join("code")).is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    // Run only in a child test process to simulate real launcher exit behavior.
    #[test]
    fn launcher_fixture() {
        match env::var("REMOTE_OCP_TEST_LAUNCH").as_deref() {
            #[cfg(windows)]
            Ok("console-probe") => {
                use std::io::IsTerminal;
                let handles = [
                    std::io::stdin().is_terminal(),
                    std::io::stdout().is_terminal(),
                    std::io::stderr().is_terminal(),
                ];
                std::fs::write(
                    env::var_os("REMOTE_OCP_TEST_CONSOLE_REPORT").unwrap(),
                    serde_json::to_vec(&handles).unwrap(),
                )
                .unwrap();
            }
            Ok("fail") => std::process::exit(7),
            Ok("late-fail") => {
                std::thread::sleep(Duration::from_secs(3));
                std::process::exit(9);
            }
            _ => {}
        }
    }

    fn fixture_command(case: &str) -> Command {
        let mut cmd = Command::new(env::current_exe().unwrap());
        cmd.args(["--exact", "remote_tools::tests::launcher_fixture"])
            .env("REMOTE_OCP_TEST_LAUNCH", case);
        cmd
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn powershell_fallback_restores_native_console_handles() {
        // Exercise real cmd.exe -> Windows PowerShell -> native-child launches,
        // including paths that must survive both command interpreters unchanged.
        let root = env::temp_dir().join(format!(
            "remote-ocp-console 한글 ' & %PATH% ! ^ (test)-{}",
            Uuid::new_v4()
        ));
        private_directory(&root).unwrap();
        let fixture = root.join("console probe.exe");
        std::fs::copy(env::current_exe().unwrap(), &fixture).unwrap();
        let report = root.join("handles.json");
        let script = format!(
            "& {} --exact remote_tools::tests::launcher_fixture --nocapture; exit $LASTEXITCODE",
            powershell_quote(&fixture.to_string_lossy())
        );
        for piped in [false, true] {
            let mut command = windows_console_command(&script).unwrap();
            command
                .env("REMOTE_OCP_TEST_LAUNCH", "console-probe")
                .env("REMOTE_OCP_TEST_CONSOLE_REPORT", &report)
                .stdin(Stdio::null())
                .stdout(if piped { Stdio::piped() } else { Stdio::null() })
                .stderr(if piped { Stdio::piped() } else { Stdio::null() })
                .kill_on_drop(true);
            let mut child = command.spawn().unwrap();
            let result = timeout(Duration::from_secs(30), child.wait()).await;
            if result.is_err() {
                // Only terminate this test's process tree, including the new
                // console, if a regression leaves PowerShell waiting forever.
                if let Some(pid) = child.id() {
                    let _ = Command::new("taskkill.exe")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .output()
                        .await;
                }
            }
            let status = result.expect("Console fixture timed out").unwrap();
            assert!(status.success(), "Console fixture failed: {status}");
            let handles: [bool; 3] =
                serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
            assert_eq!(
                handles,
                [true, true, true],
                "Native child must receive console stdin/stdout/stderr (piped parent: {piped})"
            );
            std::fs::remove_file(&report).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn launcher_checks_immediate_exit_and_reports_late_failure() {
        assert!(
            launch_detached(&mut fixture_command("fail"), "Test app", |_| {})
                .await
                .is_err()
        );
        assert!(
            launch_detached(&mut fixture_command("success"), "Test app", |_| {})
                .await
                .is_ok()
        );
        let (send, receive) = tokio::sync::oneshot::channel();
        launch_detached(&mut fixture_command("late-fail"), "Test app", |message| {
            let _ = send.send(message);
        })
        .await
        .unwrap();
        let message = timeout(Duration::from_secs(10), receive)
            .await
            .unwrap()
            .unwrap();
        assert!(message.contains("Test app"));
    }

    #[test]
    fn host_keys_stay_stable_across_local_ports_and_separate_labs() {
        let first = route("workstation.lab.example.com", 22);
        let mut next = first.clone();
        next.socks_port = Some(56789);
        assert_eq!(first.host_key_alias(), next.host_key_alias());
        next.scope = "lab-b".into();
        assert_ne!(first.host_key_alias(), next.host_key_alias());
        next = first.clone();
        next.port = 2222;
        assert_ne!(first.host_key_alias(), next.host_key_alias());
    }

    #[test]
    fn remote_ssh_config_editor_alone_does_not_satisfy_dependency() {
        let root = env::temp_dir().join(format!("remote-ocp-extensions-{}", Uuid::new_v4()));
        let editor = root.join("ms-vscode-remote.remote-ssh-edit-1.0");
        private_directory(&editor).unwrap();
        private_file(
            &editor.join("package.json"),
            br#"{"name":"remote-ssh-edit","publisher":"ms-vscode-remote"}"#,
            false,
        )
        .unwrap();
        assert!(!has_remote_ssh(&root));
        let extension = root.join("ms-vscode-remote.remote-ssh-1.0");
        private_directory(&extension).unwrap();
        private_file(
            &extension.join("package.json"),
            br#"{"name":"remote-ssh","publisher":"ms-vscode-remote"}"#,
            false,
        )
        .unwrap();
        assert!(has_remote_ssh(&root));
        private_file(
            &root.join(".obsolete"),
            br#"{"ms-vscode-remote.remote-ssh-1.0":true}"#,
            false,
        )
        .unwrap();
        assert!(!has_remote_ssh(&root));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_ssh_config_injection_and_encodes_folder_paths() {
        for user in [
            "-oProxyCommand=x",
            "student\nProxyCommand x",
            "a b",
            "$(x)",
            "",
        ] {
            assert!(validate_user(user).is_err());
        }
        for host in ["host/path", "host:22", "-oOption", "host\nUser root"] {
            assert!(!valid_target(host));
        }
        assert!(valid_target("workstation.lab.example.com"));
        assert!(valid_target("::1"));
        assert!(config_quote("/tmp/key\nProxyCommand x").is_err());
        assert!(folder_uri("example", "relative/path").is_err());
        let uri = folder_uri("example", "/home/student/my project#1?x%20").unwrap();
        assert_eq!(
            uri,
            "vscode-remote://ssh-remote+example/home/student/my%20project%231%3Fx%2520"
        );
        assert_eq!(
            powershell_quote("C:\\User's files\\$(x)"),
            "'C:\\User''s files\\$(x)'"
        );
        assert_eq!(shell_quote("a'b $(x)"), "'a'\"'\"'b $(x)'");
    }

    #[test]
    fn generated_config_is_accepted_by_openssh() {
        let directory = env::temp_dir().join(format!("remote-ocp-config-{}", Uuid::new_v4()));
        private_directory(&directory).unwrap();
        let config = directory.join("config");
        let known = directory.join("known hosts");
        private_file(
            &config,
            ssh_config("test-lab", 34567, "student", "", &known, "stable-test-host")
                .unwrap()
                .as_bytes(),
            false,
        )
        .unwrap();
        let result = std::process::Command::new(find_ssh().unwrap())
            .arg("-G")
            .arg("-F")
            .arg(&config)
            .arg("test-lab")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8_lossy(&result.stdout);
        for expected in [
            "hostname 127.0.0.1",
            "user student",
            "port 34567",
            "hostkeyalias stable-test-host",
            "stricthostkeychecking ask",
            "forwardagent no",
        ] {
            assert!(text.contains(expected), "Missing {expected}");
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn bridge_forwards_both_directions_and_disconnect_closes_clients() {
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let remote_port = server.local_addr().unwrap().port();
        let echo = tokio::spawn(async move {
            let (mut stream, _) = server.accept().await.unwrap();
            let (mut read, mut write) = stream.split();
            tokio::io::copy(&mut read, &mut write).await.unwrap();
        });
        let local = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = local.local_addr().unwrap();
        assert_eq!(address.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        let (stop, stopped) = watch::channel(false);
        let bridge = tokio::spawn(forward(local, route("127.0.0.1", remote_port), stopped));
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(b"SSH traffic").await.unwrap();
        let mut buffer = [0; 11];
        timeout(Duration::from_secs(2), client.read_exact(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer, b"SSH traffic");
        stop.send(true).unwrap();
        timeout(Duration::from_secs(2), bridge)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), client.read(&mut buffer))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert!(TcpStream::connect(address).await.is_err());
        timeout(Duration::from_secs(2), echo)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn session_shutdown_removes_launch_files_and_listener() {
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = server.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut stream, _) = server.accept().await.unwrap();
            stream
                .write_all(b"Welcome\r\nSSH-2.0-test\r\n")
                .await
                .unwrap();
        });
        let root = env::temp_dir().join(format!("remote-ocp-session-{}", Uuid::new_v4()));
        let (_stop, stopped) = watch::channel(false);
        let session = Session::prepare(&root, route("127.0.0.1", port), "student", "", stopped)
            .await
            .unwrap();
        let directory = session.directory.clone();
        let config = std::fs::read_to_string(&session.config).unwrap();
        let port: u16 = config
            .lines()
            .find_map(|line| line.trim().strip_prefix("Port "))
            .unwrap()
            .parse()
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&session.config)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        timeout(Duration::from_secs(5), session.shutdown())
            .await
            .unwrap();
        assert!(!directory.exists());
        // Rebind proves the listener was released, without waiting for Windows'
        // TCP connection-refused retries on a closed port.
        let rebound = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        drop(rebound);
        task.await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
