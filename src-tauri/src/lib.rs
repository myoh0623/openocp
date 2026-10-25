mod rhtlc;

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
    sync::{Mutex, mpsc, watch},
    task::JoinSet,
    time::timeout,
};
use url::Url;
use uuid::Uuid;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionConfig {
    #[serde(default = "default_connection_mode")]
    connection_mode: String,
    #[serde(default)]
    rhtlc_url: String,
    #[serde(default)]
    rhtlc_path: String,
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
    browser_language: String,
    keep_temporary_profile: bool,
    insecure_tls: bool,
}

fn default_connection_mode() -> String {
    "ssh".into()
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DetectedBrowser {
    name: String,
    path: PathBuf,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserCountEvent {
    count: usize,
}

#[derive(Clone)]
struct BrowserLaunchOptions {
    executable: PathBuf,
    socks_port: u16,
    dashboard_url: String,
    browser_language: String,
    insecure_tls: bool,
}

struct BrowserInstance {
    child: Child,
    temporary_profile: Option<PathBuf>,
    keep_profile: bool,
}

struct RunningTunnel {
    stop: watch::Sender<bool>,
    browser_tx: mpsc::UnboundedSender<BrowserInstance>,
    browser_options: BrowserLaunchOptions,
    keep_profile: bool,
}

#[derive(Default)]
struct AppState {
    start_lock: Mutex<()>,
    pending_stop: Mutex<Option<watch::Sender<bool>>>,
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
    config: ConnectionConfig,
) -> std::result::Result<ConnectionStarted, String> {
    let _guard = state
        .start_lock
        .try_lock()
        .map_err(|_| "연결을 준비하거나 종료하는 중입니다.".to_string())?;
    let (stop, cancel) = watch::channel(false);
    *state.pending_stop.lock().await = Some(stop);
    let result = start_connection_inner(app, &state, config, cancel).await;
    state.pending_stop.lock().await.take();
    result
}

async fn start_connection_inner(
    app: AppHandle,
    state: &AppState,
    mut config: ConnectionConfig,
    mut cancel: watch::Receiver<bool>,
) -> std::result::Result<ConnectionStarted, String> {
    validate_config(&mut config).map_err(display_error)?;

    {
        let runtime = state.runtime.lock().await;
        if runtime.is_some() {
            return Err("이미 연결이 실행 중입니다.".into());
        }
    }

    if config.connection_mode == "rhtlc" {
        return start_rhtlc_connection(app, state, config, cancel)
            .await
            .map_err(display_error);
    }

    emit_status(&app, "connecting", "SSH 인증을 확인하는 중…");
    emit_log(
        &app,
        format!(
            "SSH 서버에 연결합니다: {}@{}:{}",
            config.ssh_user, config.ssh_ip, config.ssh_port
        ),
    );

    let ssh = tokio::select! {
        result = connect_ssh(&app, &config) => result.map_err(display_error)?,
        _ = cancel.changed() => return Err("연결을 취소했습니다.".into()),
    };
    config.password.clear();
    config.private_key_passphrase.clear();
    emit_log(&app, "SSH 인증이 완료되었습니다.");

    emit_status(&app, "connecting", "대시보드 주소를 확인하는 중…");
    let dashboard_url = tokio::select! {
        result = discover_dashboard(&app, &ssh, &config.dashboard_url) => result.map_err(display_error)?,
        _ = cancel.changed() => return Err("연결을 취소했습니다.".into()),
    };

    if *cancel.borrow() {
        return Err("연결을 취소했습니다.".into());
    }
    let (listener, socks_port) = bind_socks_listener(config.first_socks_port)
        .await
        .map_err(display_error)?;
    emit_log(
        &app,
        format!("SOCKS 프록시가 127.0.0.1:{socks_port}에서 시작되었습니다."),
    );

    let detected_browser = find_browser(&config.browser_path).map_err(display_error)?;
    emit_log(
        &app,
        format!(
            "브라우저를 사용합니다: {} ({})",
            detected_browser.name,
            detected_browser.path.display()
        ),
    );
    let (profile_path, temporary_profile) =
        prepare_browser_profile(&config.browser_profile_path).map_err(display_error)?;
    let browser_options = BrowserLaunchOptions {
        executable: detected_browser.path,
        socks_port,
        dashboard_url: dashboard_url.clone(),
        browser_language: config.browser_language.clone(),
        insecure_tls: config.insecure_tls,
    };
    let browser = launch_browser(
        &browser_options.executable,
        &profile_path,
        browser_options.socks_port,
        &browser_options.dashboard_url,
        &browser_options.browser_language,
        browser_options.insecure_tls,
    )
    .map_err(display_error)?;

    emit_log(
        &app,
        format!("브라우저에서 대시보드를 엽니다: {dashboard_url}"),
    );
    if config.insecure_tls || is_education_domain(&dashboard_url) {
        emit_log(&app, "이 세션에서는 교육용 인증서 오류를 허용합니다.");
    }

    let keep_profile = config.keep_temporary_profile;
    let initial_browser = BrowserInstance {
        child: browser,
        temporary_profile: temporary_profile.then_some(profile_path),
        keep_profile,
    };
    let (stop_tx, stop_rx) = watch::channel(false);
    let (browser_tx, browser_rx) = mpsc::unbounded_channel();
    {
        let mut runtime = state.runtime.lock().await;
        *runtime = Some(RunningTunnel {
            stop: stop_tx.clone(),
            browser_tx,
            browser_options,
            keep_profile,
        });
    }

    if *cancel.borrow() {
        let _ = stop_tx.send(true);
    }
    let ssh = Arc::new(ssh);
    let background_app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_proxy_loop(
            background_app,
            listener,
            ssh,
            initial_browser,
            browser_rx,
            stop_tx,
            stop_rx,
        )
        .await;
    });

    Ok(ConnectionStarted {
        socks_port,
        dashboard_url,
    })
}

#[tauri::command]
async fn open_browser_session(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<(), String> {
    let (browser_tx, browser_options, keep_profile) = {
        let runtime = state.runtime.lock().await;
        let running = runtime
            .as_ref()
            .ok_or_else(|| "활성 연결이 없습니다.".to_string())?;
        (
            running.browser_tx.clone(),
            running.browser_options.clone(),
            running.keep_profile,
        )
    };

    let (profile_path, temporary_profile) = prepare_browser_profile("").map_err(display_error)?;
    let child = launch_browser(
        &browser_options.executable,
        &profile_path,
        browser_options.socks_port,
        &browser_options.dashboard_url,
        &browser_options.browser_language,
        browser_options.insecure_tls,
    )
    .map_err(display_error)?;
    let instance = BrowserInstance {
        child,
        temporary_profile: temporary_profile.then_some(profile_path.clone()),
        keep_profile,
    };

    if let Err(error) = browser_tx.send(instance) {
        let mut instance = error.0;
        let _ = instance.child.kill().await;
        cleanup_browser_profile(&app, instance.temporary_profile, instance.keep_profile);
        return Err("연결이 종료되어 새 브라우저 세션을 열 수 없습니다.".into());
    }

    emit_log(
        &app,
        format!("독립 브라우저 세션을 엽니다: {}", profile_path.display()),
    );
    Ok(())
}

#[tauri::command]
fn detect_browser(requested: String) -> std::result::Result<DetectedBrowser, String> {
    find_browser(requested.trim()).map_err(display_error)
}

#[tauri::command]
async fn stop_connection(state: State<'_, AppState>) -> std::result::Result<(), String> {
    if let Some(stop) = state.pending_stop.lock().await.as_ref() {
        let _ = stop.send(true);
    }
    if let Some(running) = state.runtime.lock().await.as_ref() {
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
    config.browser_language = config.browser_language.trim().to_string();
    config.private_key_path = config.private_key_path.trim().to_string();

    if config.connection_mode == "ssh" {
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
    } else if config.connection_mode == "rhtlc" {
        config.rhtlc_url = config.rhtlc_url.trim().to_string();
        config.rhtlc_path = config.rhtlc_path.trim().to_string();
        rhtlc::validate_url(&config.rhtlc_url)?;
    } else {
        bail!("지원하지 않는 접속 방식입니다.");
    }
    if !(1024..=65515).contains(&config.first_socks_port) {
        bail!("SOCKS 포트는 1024~65515 사이여야 합니다.");
    }
    browser_locale(&config.browser_language)?;
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
    initial_browser: BrowserInstance,
    mut browser_rx: mpsc::UnboundedReceiver<BrowserInstance>,
    stop_tx: watch::Sender<bool>,
    mut stop: watch::Receiver<bool>,
) {
    let mut browsers = JoinSet::new();
    spawn_browser_watcher(&mut browsers, app.clone(), initial_browser, stop.clone());
    emit_browser_count(&app, browsers.len());

    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    emit_log(&app, "사용자 요청으로 연결을 종료합니다.");
                    break;
                }
            }
            Some(browser) = browser_rx.recv() => {
                spawn_browser_watcher(&mut browsers, app.clone(), browser, stop.clone());
                emit_browser_count(&app, browsers.len());
            }
            finished = browsers.join_next(), if !browsers.is_empty() => {
                if let Some(Err(error)) = finished {
                    emit_log(&app, format!("브라우저 감시 작업 오류: {error}"));
                }
                if browsers.is_empty() {
                    if let Ok(browser) = browser_rx.try_recv() {
                        spawn_browser_watcher(&mut browsers, app.clone(), browser, stop.clone());
                        emit_browser_count(&app, browsers.len());
                        continue;
                    }
                    emit_browser_count(&app, 0);
                    emit_log(&app, "마지막 브라우저 창이 닫혀 SSH 터널을 종료합니다.");
                    break;
                }
                emit_browser_count(&app, browsers.len());
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

    let _ = stop_tx.send(true);
    while let Some(result) = browsers.join_next().await {
        if let Err(error) = result {
            emit_log(&app, format!("브라우저 종료 작업 오류: {error}"));
        }
    }
    while let Ok(mut pending) = browser_rx.try_recv() {
        let _ = pending.child.start_kill();
        cleanup_browser_profile(&app, pending.temporary_profile, pending.keep_profile);
    }
    emit_browser_count(&app, 0);

    let _ = ssh
        .disconnect(Disconnect::ByApplication, "Remote OCP closed", "ko")
        .await;

    app.state::<AppState>().runtime.lock().await.take();
    emit_status(&app, "stopped", "연결 안 됨");
}

fn spawn_browser_watcher(
    browsers: &mut JoinSet<()>,
    app: AppHandle,
    browser: BrowserInstance,
    stop: watch::Receiver<bool>,
) {
    browsers.spawn(watch_browser(app, browser, stop));
}

async fn watch_browser(
    app: AppHandle,
    mut browser: BrowserInstance,
    mut stop: watch::Receiver<bool>,
) {
    if *stop.borrow() {
        let _ = browser.child.kill().await;
        cleanup_browser_profile(&app, browser.temporary_profile, browser.keep_profile);
        return;
    }
    tokio::select! {
        changed = stop.changed() => {
            if changed.is_err() || *stop.borrow() {
                let _ = browser.child.kill().await;
            }
        }
        status = browser.child.wait() => {
            match status {
                Ok(_) => emit_log(&app, "브라우저 창이 닫혔습니다."),
                Err(error) => emit_log(&app, format!("브라우저 상태 확인 오류: {error}")),
            }
        }
    }
    cleanup_browser_profile(&app, browser.temporary_profile, browser.keep_profile);
}

fn cleanup_browser_profile(
    app: &AppHandle,
    temporary_profile: Option<PathBuf>,
    keep_profile: bool,
) {
    let Some(path) = temporary_profile else {
        return;
    };
    if keep_profile {
        emit_log(
            app,
            format!("임시 브라우저 프로필을 보관했습니다: {}", path.display()),
        );
    } else if let Err(error) = std::fs::remove_dir_all(&path)
        && path.exists()
    {
        emit_log(app, format!("임시 프로필 정리 오류: {error}"));
    }
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

fn find_browser(requested: &str) -> Result<DetectedBrowser> {
    if !requested.is_empty() {
        let path = expand_home(requested);
        if path.is_file() {
            return Ok(DetectedBrowser {
                name: browser_name_from_path(&path).to_string(),
                path,
            });
        }
        bail!(
            "선택한 브라우저 실행 파일을 찾을 수 없습니다: {}",
            path.display()
        );
    }

    let mut candidates = Vec::new();
    if cfg!(target_os = "macos") {
        let application_roots = [
            PathBuf::from("/Applications"),
            home_directory()
                .map(|home| home.join("Applications"))
                .unwrap_or_default(),
        ];
        for root in application_roots
            .iter()
            .filter(|root| !root.as_os_str().is_empty())
        {
            candidates.extend([
                DetectedBrowser {
                    name: "Google Chrome".into(),
                    path: root.join("Google Chrome.app/Contents/MacOS/Google Chrome"),
                },
                DetectedBrowser {
                    name: "Google Chrome Beta".into(),
                    path: root.join("Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta"),
                },
                DetectedBrowser {
                    name: "Chromium".into(),
                    path: root.join("Chromium.app/Contents/MacOS/Chromium"),
                },
                DetectedBrowser {
                    name: "Microsoft Edge".into(),
                    path: root.join("Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
                },
            ]);
        }
    } else if cfg!(target_os = "windows") {
        for root in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(value) = env::var_os(root) {
                let base = PathBuf::from(value);
                candidates.extend([
                    DetectedBrowser {
                        name: "Google Chrome".into(),
                        path: base.join("Google/Chrome/Application/chrome.exe"),
                    },
                    DetectedBrowser {
                        name: "Google Chrome Beta".into(),
                        path: base.join("Google/Chrome Beta/Application/chrome.exe"),
                    },
                    DetectedBrowser {
                        name: "Chromium".into(),
                        path: base.join("Chromium/Application/chrome.exe"),
                    },
                    DetectedBrowser {
                        name: "Microsoft Edge".into(),
                        path: base.join("Microsoft/Edge/Application/msedge.exe"),
                    },
                ]);
            }
        }
    } else {
        candidates.extend([
            DetectedBrowser {
                name: "Google Chrome".into(),
                path: PathBuf::from("/usr/bin/google-chrome-stable"),
            },
            DetectedBrowser {
                name: "Google Chrome".into(),
                path: PathBuf::from("/usr/bin/google-chrome"),
            },
            DetectedBrowser {
                name: "Chromium".into(),
                path: PathBuf::from("/usr/bin/chromium"),
            },
            DetectedBrowser {
                name: "Chromium".into(),
                path: PathBuf::from("/usr/bin/chromium-browser"),
            },
            DetectedBrowser {
                name: "Microsoft Edge".into(),
                path: PathBuf::from("/usr/bin/microsoft-edge-stable"),
            },
            DetectedBrowser {
                name: "Microsoft Edge".into(),
                path: PathBuf::from("/usr/bin/microsoft-edge"),
            },
        ]);
    }

    if let Some(path_value) = env::var_os("PATH") {
        let executable_names: &[(&str, &str)] = if cfg!(target_os = "windows") {
            &[
                ("Google Chrome", "chrome.exe"),
                ("Chromium", "chromium.exe"),
                ("Microsoft Edge", "msedge.exe"),
            ]
        } else {
            &[
                ("Google Chrome", "google-chrome"),
                ("Google Chrome", "google-chrome-stable"),
                ("Chromium", "chromium"),
                ("Chromium", "chromium-browser"),
                ("Microsoft Edge", "microsoft-edge"),
                ("Microsoft Edge", "microsoft-edge-stable"),
            ]
        };
        for directory in env::split_paths(&path_value) {
            for (name, executable) in executable_names {
                candidates.push(DetectedBrowser {
                    name: (*name).into(),
                    path: directory.join(executable),
                });
            }
        }
    }

    candidates
        .into_iter()
        .find(|browser| browser.path.is_file())
        .ok_or_else(|| {
            anyhow!(
                "Chrome, Chromium 또는 Edge를 찾지 못했습니다. 실행 파일 경로를 직접 입력해 주세요."
            )
        })
}

fn browser_name_from_path(path: &Path) -> &'static str {
    let value = path.to_string_lossy().to_ascii_lowercase();
    if value.contains("msedge") || value.contains("microsoft edge") {
        "Microsoft Edge"
    } else if value.contains("chromium") {
        "Chromium"
    } else if value.contains("chrome beta") {
        "Google Chrome Beta"
    } else if value.contains("chrome") {
        "Google Chrome"
    } else {
        "Custom Chromium browser"
    }
}

fn browser_locale(language: &str) -> Result<(&'static str, &'static str)> {
    match language {
        "en-US" => Ok(("en-US", "en-US,en")),
        "ko-KR" => Ok(("ko-KR", "ko-KR,ko,en-US,en")),
        _ => bail!("브라우저 언어는 English 또는 한국어 중에서 선택해 주세요."),
    }
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
    browser_language: &str,
    insecure_tls: bool,
) -> Result<Child> {
    let (language, accept_language) = browser_locale(browser_language)?;
    let mut command = Command::new(executable);
    command.kill_on_drop(true);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--proxy-server=socks5://127.0.0.1:{socks_port}"))
        .arg(format!("--lang={language}"))
        .arg(format!("--accept-lang={accept_language}"))
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

fn emit_browser_count(app: &AppHandle, count: usize) {
    let _ = app.emit("browser-session-count", BrowserCountEvent { count });
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            start_connection,
            open_browser_session,
            detect_browser,
            detect_rhtlc,
            stop_connection
        ])
        .build(tauri::generate_context!())
        .expect("Remote OCP 앱을 시작할 수 없습니다.");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested {
            api, code: None, ..
        } = event
        {
            api.prevent_exit();
            let app = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<AppState>();
                if let Some(stop) = state.pending_stop.lock().await.as_ref() {
                    let _ = stop.send(true);
                }
                // Wait for startup cleanup before stopping any established connection.
                let _guard = state.start_lock.lock().await;
                if let Some(running) = state.runtime.lock().await.as_ref() {
                    let _ = running.stop.send(true);
                }
                while state.runtime.lock().await.is_some() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                app.exit(0);
            });
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

    #[test]
    fn maps_supported_browser_languages() {
        assert_eq!(browser_locale("en-US").unwrap(), ("en-US", "en-US,en"));
        assert_eq!(
            browser_locale("ko-KR").unwrap(),
            ("ko-KR", "ko-KR,ko,en-US,en")
        );
        assert!(browser_locale("fr-FR").is_err());
    }

    #[test]
    fn accepts_a_manual_browser_executable() {
        let path = env::temp_dir().join(format!("chrome-test-{}", Uuid::new_v4()));
        std::fs::write(&path, b"test").unwrap();

        let detected = find_browser(path.to_str().unwrap()).unwrap();

        assert_eq!(detected.name, "Google Chrome");
        assert_eq!(detected.path, path);
        std::fs::remove_file(detected.path).unwrap();
    }
}

#[tauri::command]
fn detect_rhtlc(requested: String) -> std::result::Result<PathBuf, String> {
    rhtlc::find_cli(requested.trim()).map_err(display_error)
}

async fn start_rhtlc_connection(
    app: AppHandle,
    state: &AppState,
    mut config: ConnectionConfig,
    mut cancel: watch::Receiver<bool>,
) -> Result<ConnectionStarted> {
    let cli = rhtlc::find_cli(&config.rhtlc_path)?;
    let browser = find_browser(&config.browser_path)?;
    let dashboard = if config.dashboard_url.is_empty() {
        "https://console-openshift-console.apps.ocp4.example.com/".to_string()
    } else {
        config.dashboard_url.clone()
    };
    let (reservation, port) = bind_socks_listener(config.first_socks_port).await?;
    drop(reservation);
    let mut tunnel = rhtlc::Connection::new(cli, port);
    emit_status(&app, "connecting", "RHTLC로 실습 환경에 연결하는 중…");
    emit_log(
        &app,
        "공식 RHTLC 클라이언트로 연결합니다. 주소의 인증 토큰은 기록하지 않습니다.",
    );
    let preparation = {
        let work = async {
            tunnel.connect(&config.rhtlc_url).await?;
            emit_status(&app, "connecting", "RHTLC 대시보드 접속을 확인하는 중…");
            rhtlc::probe(tunnel.port, &dashboard).await
        };
        tokio::pin!(work);
        tokio::select! {
            result = &mut work => result,
            _ = cancel.changed() => {
                // Let the bounded CLI operation register its daemon before disconnecting
                // our SKU. Killing the CLI mid-startup could orphan that daemon.
                let _ = work.await;
                Err(anyhow!("연결을 취소했습니다."))
            },
        }
    };
    config.rhtlc_url.clear();
    if let Err(error) = preparation {
        if let Err(cleanup) = tunnel.disconnect().await {
            emit_log(&app, cleanup.to_string());
        }
        return Err(error);
    }
    let options = BrowserLaunchOptions {
        executable: browser.path,
        socks_port: tunnel.port,
        dashboard_url: dashboard.clone(),
        browser_language: config.browser_language,
        insecure_tls: config.insecure_tls,
    };
    let launched = (|| -> Result<BrowserInstance> {
        let (profile, temporary) = prepare_browser_profile(&config.browser_profile_path)?;
        match launch_browser(
            &options.executable,
            &profile,
            options.socks_port,
            &options.dashboard_url,
            &options.browser_language,
            options.insecure_tls,
        ) {
            Ok(child) => Ok(BrowserInstance {
                child,
                temporary_profile: temporary.then_some(profile),
                keep_profile: config.keep_temporary_profile,
            }),
            Err(error) => {
                cleanup_browser_profile(&app, temporary.then_some(profile), false);
                Err(error)
            }
        }
    })();
    let initial = match launched {
        Ok(browser) => browser,
        Err(error) => {
            let _ = tunnel.disconnect().await;
            return Err(error);
        }
    };
    let (stop, stopped) = watch::channel(false);
    let (sender, receiver) = mpsc::unbounded_channel();
    let port = tunnel.port;
    *state.runtime.lock().await = Some(RunningTunnel {
        stop: stop.clone(),
        browser_tx: sender,
        browser_options: options,
        keep_profile: config.keep_temporary_profile,
    });
    if *cancel.borrow() {
        let _ = stop.send(true);
    }
    emit_log(
        &app,
        format!("RHTLC 프록시 연결 확인 완료 · 127.0.0.1:{port}"),
    );
    tauri::async_runtime::spawn(watch_rhtlc(app, tunnel, initial, receiver, stop, stopped));
    Ok(ConnectionStarted {
        socks_port: port,
        dashboard_url: dashboard,
    })
}

async fn watch_rhtlc(
    app: AppHandle,
    tunnel: rhtlc::Connection,
    initial: BrowserInstance,
    mut incoming: mpsc::UnboundedReceiver<BrowserInstance>,
    stop: watch::Sender<bool>,
    mut stopped: watch::Receiver<bool>,
) {
    let mut browsers = JoinSet::new();
    spawn_browser_watcher(&mut browsers, app.clone(), initial, stopped.clone());
    emit_browser_count(&app, 1);
    let mut health = tokio::time::interval(Duration::from_secs(5));
    let mut failed = false;
    loop {
        if *stopped.borrow() {
            break;
        }
        tokio::select! {
            _ = stopped.changed() => break,
            Some(browser) = incoming.recv() => {
                spawn_browser_watcher(&mut browsers, app.clone(), browser, stopped.clone());
                emit_browser_count(&app, browsers.len());
            }
            _ = browsers.join_next(), if !browsers.is_empty() => {
                if browsers.is_empty() {
                    if let Ok(browser) = incoming.try_recv() {
                        spawn_browser_watcher(&mut browsers, app.clone(), browser, stopped.clone());
                    } else { break; }
                }
                emit_browser_count(&app, browsers.len());
            }
            _ = health.tick() => {
                if !matches!(timeout(Duration::from_secs(2), TcpStream::connect((Ipv4Addr::LOCALHOST, tunnel.port))).await, Ok(Ok(_))) {
                    failed = true;
                    emit_log(&app, "RHTLC 프록시가 종료되었습니다.");
                    break;
                }
            }
        }
    }
    let _ = stop.send(true);
    incoming.close();
    while browsers.join_next().await.is_some() {}
    while let Some(mut browser) = incoming.recv().await {
        let _ = browser.child.kill().await;
        cleanup_browser_profile(&app, browser.temporary_profile, browser.keep_profile);
    }
    if let Err(error) = tunnel.disconnect().await {
        failed = true;
        emit_log(
            &app,
            format!("{error} 공식 RHTLC 앱에서 remote-ocp 연결을 확인해 주세요."),
        );
    }
    app.state::<AppState>().runtime.lock().await.take();
    emit_browser_count(&app, 0);
    if !failed {
        emit_log(&app, "Remote OCP에서 만든 RHTLC 연결을 정리했습니다.");
    }
    emit_status(
        &app,
        if failed { "error" } else { "stopped" },
        if failed {
            "RHTLC 연결이 끊어졌거나 종료에 실패했습니다. 실행 기록을 확인해 주세요."
        } else {
            "연결 안 됨"
        },
    );
}
