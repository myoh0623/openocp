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
    bridge: tokio::task::JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.bridge.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl Session {
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
            bridge,
        })
    }

    pub async fn open_terminal(&self) -> Result<()> {
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
            let status = timeout(
                Duration::from_secs(10),
                Command::new("/usr/bin/open")
                    .args(["-a", "Terminal"])
                    .arg(&script)
                    .status(),
            )
            .await??;
            if !status.success() {
                tr_bail!("Terminal could not be opened.", "터미널을 열지 못했습니다.");
            }
            return Ok(());
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
                )?;
            } else {
                let text = format!(
                    "& {} -F {} {}",
                    powershell_quote(&self.ssh.to_string_lossy()),
                    powershell_quote(&self.config.to_string_lossy()),
                    powershell_quote(&self.alias)
                );
                let powershell = env::var_os("SystemRoot")
                    .map(PathBuf::from)
                    .context("SystemRoot is unavailable")?
                    .join("System32/WindowsPowerShell/v1.0/powershell.exe");
                launch_detached(
                    Command::new(powershell)
                        .args(["-NoLogo", "-NoProfile", "-NoExit", "-Command"])
                        .arg(text)
                        .creation_flags(0x00000010),
                )?;
            }
            return Ok(());
        }
        #[allow(unreachable_code)]
        Err(anyhow!(tr!(
            "External terminals are supported on macOS and Windows.",
            "외부 터미널은 macOS와 Windows에서 지원합니다."
        )))
    }

    pub fn open_vscode(&self, root: &Path, requested: &str, folder: &str) -> Result<()> {
        let code = find_vscode(requested)?;
        let extensions = home_directory()
            .context("Home directory is unavailable")?
            .join(".vscode/extensions");
        if !has_remote_ssh(&extensions) {
            tr_bail!(
                "Install Microsoft's Remote - SSH extension in VS Code, then try again.",
                "VS Code에서 Microsoft의 Remote - SSH 확장을 설치한 뒤 다시 시도하세요."
            );
        }
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
        let mut settings: serde_json::Map<String, serde_json::Value> = if settings_path.exists() {
            serde_json::from_slice(&std::fs::read(&settings_path)?).context(tr!(
                "Cannot read the dedicated VS Code settings file.",
                "전용 VS Code 설정 파일을 읽지 못했습니다."
            ))?
        } else {
            serde_json::Map::new()
        };
        settings.insert(
            "remote.SSH.configFile".into(),
            serde_json::json!(self.config),
        );
        settings.insert("remote.SSH.path".into(), serde_json::json!(self.ssh));
        settings.insert(
            "remote.SSH.showLoginTerminal".into(),
            serde_json::json!(true),
        );
        private_file(
            &settings_path,
            &serde_json::to_vec_pretty(&settings)?,
            false,
        )?;
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
        launch_detached(&mut command)
    }
}

fn launch_detached(command: &mut Command) -> Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context(tr!(
            "Could not launch the external application.",
            "외부 프로그램을 실행하지 못했습니다."
        ))?;
    // Launchers may exit while an existing application takes over. Never kill
    // the user's editor or infer the tunnel lifetime from this process.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
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

fn has_remote_ssh(extensions: &Path) -> bool {
    std::fs::read_dir(extensions).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let candidate = entry
                .file_name()
                .to_string_lossy()
                .starts_with("ms-vscode-remote.remote-ssh-");
            candidate
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
    async fn session_drop_removes_launch_files_and_listener() {
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
        drop(session);
        assert!(!directory.exists());
        timeout(Duration::from_secs(2), async {
            while TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                .await
                .is_ok()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
