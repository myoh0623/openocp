use std::{
    env,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use russh::{
    ChannelMsg, Disconnect, client,
    keys::{
        PrivateKeyWithHashAlg, PublicKeyOrCertificate, check_known_hosts,
        known_hosts::learn_known_hosts, load_secret_key,
    },
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    sync::{Mutex, watch},
    time::timeout,
};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionConfig {
    ssh_ip: String,
    ssh_port: u16,
    ssh_user: String,
    password: String,
    private_key_path: String,
    private_key_passphrase: String,
    first_socks_port: u16,
    dashboard_url: String,
    browser_path: String,
    browser_profile_path: String,
    keep_temporary_profile: bool,
    insecure_tls: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionStarted {
    socks_port: u16,
    dashboard_url: String,
}

#[derive(Clone, Serialize)]
struct StatusEvent {
    state: &'static str,
    message: String,
}

struct RunningTunnel {
    stop: watch::Sender<bool>,
}

#[derive(Default)]
struct AppState {
    runtime: Mutex<Option<RunningTunnel>>,
}

struct SshClient {
    host: String,
    port: u16,
}

impl client::Handler for SshClient {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let public_key = server_public_key.public_key();
        match check_known_hosts(&self.host, self.port, &public_key) {
            Ok(true) => Ok(true),
            Ok(false) => {
                learn_known_hosts(&self.host, self.port, &public_key)?;
                Ok(true)
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[tauri::command]
async fn start_connection(
    app: AppHandle,
    state: State<'_, AppState>,
    mut config: ConnectionConfig,
) -> std::result::Result<ConnectionStarted, String> {
    validate_config(&mut config).map_err(display_error)?;

    {
        let runtime = state.runtime.lock().await;
        if runtime.is_some() {
            return Err("이미 연결이 실행 중입니다.".into());
        }
    }

    emit_status(&app, "connecting", "SSH 인증을 확인하는 중…");
    emit_log(
        &app,
        format!(
            "SSH 서버에 연결합니다: {}@{}:{}",
            config.ssh_user, config.ssh_ip, config.ssh_port
        ),
    );

    let ssh = connect_ssh(&app, &config).await.map_err(display_error)?;
    config.password.clear();
    config.private_key_passphrase.clear();
    emit_log(&app, "SSH 인증이 완료되었습니다.");

    emit_status(&app, "connecting", "대시보드 주소를 확인하는 중…");
    let dashboard_url = discover_dashboard(&app, &ssh, &config.dashboard_url)
        .await
        .map_err(display_error)?;

    let (listener, socks_port) = bind_socks_listener(config.first_socks_port)
        .await
        .map_err(display_error)?;
    emit_log(
        &app,
        format!("SOCKS 프록시가 127.0.0.1:{socks_port}에서 시작되었습니다."),
    );

    let browser_path = find_browser(&config.browser_path).map_err(display_error)?;
    let (profile_path, temporary_profile) =
        prepare_browser_profile(&config.browser_profile_path).map_err(display_error)?;
    let browser = launch_browser(
        &browser_path,
        &profile_path,
        socks_port,
        &dashboard_url,
        config.insecure_tls,
    )
    .map_err(display_error)?;

    emit_log(
        &app,
        format!("브라우저에서 대시보드를 엽니다: {dashboard_url}"),
    );
    if config.insecure_tls || is_education_domain(&dashboard_url) {
        emit_log(&app, "이 세션에서는 교육용 인증서 오류를 허용합니다.");
    }

    let (stop_tx, stop_rx) = watch::channel(false);
    {
        let mut runtime = state.runtime.lock().await;
        *runtime = Some(RunningTunnel { stop: stop_tx });
    }

    let ssh = Arc::new(ssh);
    let background_app = app.clone();
    let keep_profile = config.keep_temporary_profile;
    tauri::async_runtime::spawn(async move {
        run_proxy_loop(
            background_app,
            listener,
            ssh,
            browser,
            stop_rx,
            temporary_profile.then_some(profile_path),
            keep_profile,
        )
        .await;
    });

    Ok(ConnectionStarted {
        socks_port,
        dashboard_url,
    })
}

#[tauri::command]
async fn stop_connection(state: State<'_, AppState>) -> std::result::Result<(), String> {
    let mut runtime = state.runtime.lock().await;
    if let Some(running) = runtime.take() {
        let _ = running.stop.send(true);
    }
    Ok(())
}

fn validate_config(config: &mut ConnectionConfig) -> Result<()> {
    config.ssh_ip = config.ssh_ip.trim().to_string();
    config.ssh_user = config.ssh_user.trim().to_string();
    config.dashboard_url = config.dashboard_url.trim().to_string();
    config.browser_path = config.browser_path.trim().to_string();
    config.browser_profile_path = config.browser_profile_path.trim().to_string();
    config.private_key_path = config.private_key_path.trim().to_string();

    let parsed_ip: IpAddr = config
        .ssh_ip
        .parse()
        .context("올바른 SSH IP 주소를 입력해 주세요.")?;
    if !matches!(parsed_ip, IpAddr::V4(_)) {
        bail!("현재 버전은 IPv4 SSH 주소를 사용합니다.");
    }
    let user_pattern = Regex::new(r"^[A-Za-z0-9._-]+$")?;
    if !user_pattern.is_match(&config.ssh_user) {
        bail!("SSH 사용자 이름이 올바르지 않습니다.");
    }
    if config.ssh_port == 0 {
        bail!("SSH 포트는 1~65535 사이여야 합니다.");
    }
    if !(1024..=65515).contains(&config.first_socks_port) {
        bail!("SOCKS 포트는 1024~65515 사이여야 합니다.");
    }
    if !config.dashboard_url.is_empty() {
        let parsed = Url::parse(&config.dashboard_url)
            .context("대시보드 URL은 http:// 또는 https://로 시작해야 합니다.")?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            bail!("대시보드 URL은 http:// 또는 https://로 시작해야 합니다.");
        }
    }
    Ok(())
}

async fn connect_ssh(
    app: &AppHandle,
    config: &ConnectionConfig,
) -> Result<client::Handle<SshClient>> {
    let ssh_config = client::Config {
        inactivity_timeout: Some(Duration::from_secs(90)),
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    };
    let address = (config.ssh_ip.as_str(), config.ssh_port);
    let mut session = timeout(
        Duration::from_secs(20),
        client::connect(
            Arc::new(ssh_config),
            address,
            SshClient {
                host: config.ssh_ip.clone(),
                port: config.ssh_port,
            },
        ),
    )
    .await
    .context("SSH 서버 응답 시간이 초과되었습니다.")?
    .context("SSH 서버에 연결할 수 없습니다. IP와 포트를 확인해 주세요.")?;

    let key_path = select_private_key(&config.private_key_path);
    if let Some(path) = key_path {
        match authenticate_key(
            &mut session,
            &config.ssh_user,
            &path,
            &config.private_key_passphrase,
        )
        .await
        {
            Ok(true) => {
                emit_log(app, format!("SSH 개인 키 인증 사용: {}", path.display()));
                return Ok(session);
            }
            Ok(false) => emit_log(app, "SSH 개인 키가 거부되어 비밀번호 인증을 시도합니다."),
            Err(error) => emit_log(app, format!("SSH 개인 키를 사용할 수 없습니다: {error}")),
        }
    }

    if config.password.is_empty() {
        bail!("사용 가능한 SSH 개인 키가 없습니다. SSH 비밀번호를 입력해 주세요.");
    }

    let auth = session
        .authenticate_password(&config.ssh_user, &config.password)
        .await
        .context("SSH 비밀번호 인증을 완료할 수 없습니다.")?;
    if !auth.success() {
        bail!("SSH 인증에 실패했습니다. 사용자 이름과 비밀번호를 확인해 주세요.");
    }
    emit_log(app, "SSH 비밀번호 인증을 사용했습니다.");
    Ok(session)
}

async fn authenticate_key(
    session: &mut client::Handle<SshClient>,
    user: &str,
    path: &Path,
    passphrase: &str,
) -> Result<bool> {
    let secret = if passphrase.is_empty() {
        None
    } else {
        Some(passphrase)
    };
    let key = load_secret_key(path, secret).context("개인 키 파일을 읽지 못했습니다.")?;
    let hash = session.best_supported_rsa_hash().await?.flatten();
    let auth = session
        .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
        .await?;
    Ok(auth.success())
}

fn select_private_key(requested: &str) -> Option<PathBuf> {
    if !requested.is_empty() {
        return Some(expand_home(requested));
    }
    let home = home_directory()?;
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .into_iter()
        .map(|name| home.join(".ssh").join(name))
        .find(|path| path.is_file())
}

async fn discover_dashboard(
    app: &AppHandle,
    ssh: &client::Handle<SshClient>,
    requested: &str,
) -> Result<String> {
    if !requested.is_empty() {
        return Ok(requested.to_string());
    }

    let console_url = run_remote_command(ssh, "oc whoami --show-console 2>/dev/null || true")
        .await?
        .trim()
        .to_string();
    if valid_web_url(&console_url) {
        emit_log(app, "OpenShift 콘솔 주소를 자동으로 찾았습니다.");
        return Ok(console_url);
    }

    let console_host = run_remote_command(
        ssh,
        "oc get route console -n openshift-console -o jsonpath='{.spec.host}' 2>/dev/null || true",
    )
    .await?
    .trim()
    .to_string();
    if valid_hostname(&console_host) {
        emit_log(app, "OpenShift console Route를 자동으로 찾았습니다.");
        return Ok(format!("https://{console_host}/"));
    }

    let rhoai_host = run_remote_command(
        ssh,
        "oc get route rhods-dashboard -n redhat-ods-applications -o jsonpath='{.spec.host}' 2>/dev/null || true",
    )
    .await?
    .trim()
    .to_string();
    if valid_hostname(&rhoai_host) {
        emit_log(app, "RHOAI Dashboard Route를 자동으로 찾았습니다.");
        return Ok(format!("https://{rhoai_host}/"));
    }

    emit_log(
        app,
        "콘솔 주소를 찾지 못해 현재 실습 기본 주소를 사용합니다.",
    );
    Ok("https://console-openshift-console.apps.ocp4.example.com/".into())
}

async fn run_remote_command(ssh: &client::Handle<SshClient>, command: &str) -> Result<String> {
    let mut channel = ssh.channel_open_session().await?;
    channel.exec(true, command).await?;
    let mut output = Vec::new();
    while let Some(message) = channel.wait().await {
        if let ChannelMsg::Data { data } = message {
            output.extend_from_slice(&data);
        }
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

async fn bind_socks_listener(first_port: u16) -> Result<(TcpListener, u16)> {
    for offset in 0..20u16 {
        let port = first_port + offset;
        if let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
            return Ok((listener, port));
        }
    }
    bail!(
        "{}~{} 범위에서 사용할 수 있는 SOCKS 포트를 찾지 못했습니다.",
        first_port,
        first_port + 19
    )
}

async fn run_proxy_loop(
    app: AppHandle,
    listener: TcpListener,
    ssh: Arc<client::Handle<SshClient>>,
    mut browser: Child,
    mut stop: watch::Receiver<bool>,
    temporary_profile: Option<PathBuf>,
    keep_profile: bool,
) {
    loop {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    emit_log(&app, "사용자 요청으로 연결을 종료합니다.");
                    let _ = browser.kill().await;
                    break;
                }
            }
            status = browser.wait() => {
                match status {
                    Ok(_) => emit_log(&app, "브라우저 창이 닫혀 SSH 터널을 종료합니다."),
                    Err(error) => emit_log(&app, format!("브라우저 상태 확인 오류: {error}")),
                }
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((socket, peer)) => {
                        let ssh = Arc::clone(&ssh);
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(error) = proxy_socks_connection(socket, peer, ssh).await {
                                emit_log(&app, format!("프록시 연결 오류: {error}"));
                            }
                        });
                    }
                    Err(error) => {
                        emit_log(&app, format!("SOCKS 연결 수락 오류: {error}"));
                        break;
                    }
                }
            }
        }
    }

    let _ = ssh
        .disconnect(Disconnect::ByApplication, "Remote OCP closed", "ko")
        .await;
    if let Some(path) = temporary_profile {
        if keep_profile {
            emit_log(
                &app,
                format!("임시 브라우저 프로필을 보관했습니다: {}", path.display()),
            );
        } else if let Err(error) = std::fs::remove_dir_all(&path)
            && path.exists()
        {
            emit_log(&app, format!("임시 프로필 정리 오류: {error}"));
        }
    }

    if let Ok(mut runtime) = app.state::<AppState>().runtime.try_lock() {
        *runtime = None;
    }
    emit_status(&app, "stopped", "연결 안 됨");
}

async fn proxy_socks_connection(
    mut socket: TcpStream,
    peer: std::net::SocketAddr,
    ssh: Arc<client::Handle<SshClient>>,
) -> Result<()> {
    let mut greeting = [0u8; 2];
    socket.read_exact(&mut greeting).await?;
    if greeting[0] != 5 || greeting[1] == 0 {
        bail!("지원되지 않는 SOCKS 요청입니다.");
    }
    let mut methods = vec![0u8; greeting[1] as usize];
    socket.read_exact(&mut methods).await?;
    socket.write_all(&[5, 0]).await?;

    let mut request = [0u8; 4];
    socket.read_exact(&mut request).await?;
    if request[0] != 5 || request[1] != 1 {
        socket.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        bail!("SOCKS CONNECT 이외의 명령은 지원하지 않습니다.");
    }

    let host = match request[3] {
        1 => {
            let mut octets = [0u8; 4];
            socket.read_exact(&mut octets).await?;
            Ipv4Addr::from(octets).to_string()
        }
        3 => {
            let length = socket.read_u8().await? as usize;
            let mut name = vec![0u8; length];
            socket.read_exact(&mut name).await?;
            String::from_utf8(name).context("SOCKS 호스트 이름이 UTF-8이 아닙니다.")?
        }
        4 => {
            let mut octets = [0u8; 16];
            socket.read_exact(&mut octets).await?;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        _ => bail!("지원되지 않는 SOCKS 주소 형식입니다."),
    };
    let port = socket.read_u16().await?;

    let channel = match ssh
        .channel_open_direct_tcpip(host, port as u32, peer.ip().to_string(), peer.port() as u32)
        .await
    {
        Ok(channel) => channel,
        Err(error) => {
            socket.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(error.into());
        }
    };
    socket.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    let mut channel_stream = channel.into_stream();
    tokio::io::copy_bidirectional(&mut socket, &mut channel_stream).await?;
    let _ = channel_stream.shutdown().await;
    Ok(())
}

fn find_browser(requested: &str) -> Result<PathBuf> {
    if !requested.is_empty() {
        let path = expand_home(requested);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "선택한 브라우저 실행 파일을 찾을 수 없습니다: {}",
            path.display()
        );
    }

    let mut candidates = Vec::new();
    if cfg!(target_os = "macos") {
        candidates.extend([
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            PathBuf::from("/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta"),
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
            PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
        ]);
    } else if cfg!(target_os = "windows") {
        for root in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(value) = env::var_os(root) {
                let base = PathBuf::from(value);
                candidates.push(base.join("Google/Chrome/Application/chrome.exe"));
                candidates.push(base.join("Microsoft/Edge/Application/msedge.exe"));
            }
        }
    } else {
        candidates.extend([
            PathBuf::from("/usr/bin/google-chrome"),
            PathBuf::from("/usr/bin/chromium"),
            PathBuf::from("/usr/bin/microsoft-edge"),
        ]);
    }

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            anyhow!(
                "Chrome, Chromium 또는 Edge를 찾지 못했습니다. 실행 파일 경로를 직접 입력해 주세요."
            )
        })
}

fn prepare_browser_profile(requested: &str) -> Result<(PathBuf, bool)> {
    let (path, temporary) = if requested.is_empty() {
        (
            env::temp_dir().join(format!("RemoteOCP-Chrome-{}", Uuid::new_v4())),
            true,
        )
    } else {
        (expand_home(requested), false)
    };
    std::fs::create_dir_all(&path).context("브라우저 프로필 폴더를 만들 수 없습니다.")?;
    Ok((path, temporary))
}

fn launch_browser(
    executable: &Path,
    profile: &Path,
    socks_port: u16,
    dashboard_url: &str,
    insecure_tls: bool,
) -> Result<Child> {
    let mut command = Command::new(executable);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--proxy-server=socks5://127.0.0.1:{socks_port}"))
        .arg("--test-type")
        .arg("--disable-background-networking")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--new-window");
    if insecure_tls || is_education_domain(dashboard_url) {
        command.arg("--ignore-certificate-errors");
    }
    command.arg(dashboard_url);
    command
        .spawn()
        .with_context(|| format!("브라우저를 실행할 수 없습니다: {}", executable.display()))
}

fn valid_web_url(value: &str) -> bool {
    Url::parse(value)
        .map(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        .unwrap_or(false)
}

fn valid_hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn is_education_domain(value: &str) -> bool {
    Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host.ends_with(".example.com"))
}

fn home_directory() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn expand_home(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = home_directory()
    {
        return home.join(rest);
    }
    PathBuf::from(value)
}

fn emit_log(app: &AppHandle, message: impl Into<String>) {
    let _ = app.emit("tunnel-log", message.into());
}

fn emit_status(app: &AppHandle, state: &'static str, message: impl Into<String>) {
    let _ = app.emit(
        "tunnel-status",
        StatusEvent {
            state,
            message: message.into(),
        },
    );
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![start_connection, stop_connection])
        .build(tauri::generate_context!())
        .expect("Remote OCP 앱을 시작할 수 없습니다.");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            let state = app_handle.state::<AppState>();
            if let Ok(mut runtime) = state.runtime.try_lock()
                && let Some(running) = runtime.take()
            {
                let _ = running.stop.send(true);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_expected_dashboard_addresses() {
        assert!(valid_web_url(
            "https://console-openshift-console.apps.ocp4.example.com/"
        ));
        assert!(valid_hostname("rhods-dashboard.apps.ocp4.example.com"));
        assert!(!valid_web_url("file:///tmp/dashboard"));
        assert!(!valid_hostname("dashboard name.example.com"));
    }

    #[test]
    fn detects_education_domains_only() {
        assert!(is_education_domain(
            "https://console-openshift-console.apps.ocp4.example.com"
        ));
        assert!(!is_education_domain("https://example.com.evil.invalid"));
    }
}
