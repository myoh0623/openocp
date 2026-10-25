#[macro_use]
mod language;
mod remote_tools;
mod rhtlc;

use std::{
    env,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
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
    #[serde(default)]
    remote_host: String,
    #[serde(default = "default_remote_user")]
    remote_user: String,
    #[serde(default = "default_remote_port")]
    remote_port: u16,
    #[serde(default)]
    remote_folder: String,
    #[serde(default)]
    vscode_path: String,
}

fn default_remote_user() -> String {
    "student".into()
}
fn default_remote_port() -> u16 {
    22
}

fn default_connection_mode() -> String {
    "ssh".into()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionStarted {
    socks_port: u16,
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

struct BrowserInstance {
    child: Child,
    temporary_profile: Option<PathBuf>,
    keep_profile: bool,
}

struct RunningTunnel {
    stop: watch::Sender<bool>,
    browser_tx: mpsc::UnboundedSender<BrowserInstance>,
    config: ConnectionConfig,
    socks_port: u16,
    ssh: Option<Arc<client::Handle<SshClient>>>,
    route: remote_tools::Route,
    tools: Mutex<Option<remote_tools::Session>>,
    browser_opened: Mutex<bool>,
}

#[derive(Default)]
struct AppState {
    start_lock: Mutex<()>,
    pending_stop: Mutex<Option<watch::Sender<bool>>>,
    runtime: Mutex<Option<Arc<RunningTunnel>>>,
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
    let _guard = state.start_lock.try_lock().map_err(|_| {
        tr!(
            "A connection is starting or stopping.",
            "연결을 준비하거나 종료하는 중입니다."
        )
        .to_string()
    })?;
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
    if state.runtime.lock().await.is_some() {
        return Err(tr!(
            "A connection is already running.",
            "이미 연결이 실행 중입니다."
        )
        .into());
    }
    if config.connection_mode == "rhtlc" {
        return start_rhtlc_connection(app, state, config, cancel)
            .await
            .map_err(display_error);
    }
    emit_status(
        &app,
        "connecting",
        tr!("Authenticating SSH…", "SSH 인증을 확인하는 중…"),
    );
    let ssh = tokio::select! {
        result = connect_ssh(&app, &config) => Arc::new(result.map_err(display_error)?),
        _ = cancel.changed() => return Err(tr!("Connection cancelled.", "연결을 취소했습니다.").into()),
    };
    config.password.clear();
    config.private_key_passphrase.clear();
    config.rhtlc_url.clear();
    let (listener, socks_port) = bind_socks_listener(config.first_socks_port)
        .await
        .map_err(display_error)?;
    let route = remote_tools::Route {
        host: config.ssh_ip.clone(),
        port: config.ssh_port,
        socks_port: None,
        scope: format!("ssh:{}:{}", config.ssh_ip, config.ssh_port),
    };
    let (stop, stopped) = watch::channel(false);
    let (sender, receiver) = mpsc::unbounded_channel();
    *state.runtime.lock().await = Some(Arc::new(RunningTunnel {
        stop: stop.clone(),
        browser_tx: sender,
        config,
        socks_port,
        ssh: Some(ssh.clone()),
        route,
        tools: Mutex::new(None),
        browser_opened: Mutex::new(false),
    }));
    if *cancel.borrow() {
        let _ = stop.send(true);
    }
    emit_log(
        &app,
        tr!(
            "Connected. Open a dashboard, Terminal, or VS Code. Use Disconnect when finished.",
            "연결되었습니다. 대시보드, 터미널 또는 VS Code를 열고 작업이 끝나면 연결을 종료하세요."
        ),
    );
    tauri::async_runtime::spawn(run_proxy_loop(app, listener, ssh, receiver, stop, stopped));
    Ok(ConnectionStarted { socks_port })
}

async fn active_runtime(state: &AppState) -> Result<Arc<RunningTunnel>> {
    let runtime = state.runtime.lock().await.clone().context(tr!(
        "There is no active connection.",
        "활성 연결이 없습니다."
    ))?;
    if *runtime.stop.borrow() {
        tr_bail!("The connection is stopping.", "연결을 종료하는 중입니다.");
    }
    Ok(runtime)
}

#[tauri::command]
async fn open_browser_session(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<(), String> {
    open_dashboard_inner(&app, &state)
        .await
        .map_err(display_error)
}

async fn open_dashboard_inner(app: &AppHandle, state: &AppState) -> Result<()> {
    let runtime = active_runtime(state).await?;
    let mut opened = runtime.browser_opened.lock().await;
    let config = &runtime.config;
    let mut stop = runtime.stop.subscribe();
    if *stop.borrow() {
        tr_bail!("The connection is stopping.", "연결을 종료하는 중입니다.");
    }
    let dashboard = if !config.dashboard_url.is_empty() {
        config.dashboard_url.clone()
    } else if let Some(ssh) = &runtime.ssh {
        tokio::select! {
            result = timeout(Duration::from_secs(20), discover_dashboard(app, ssh, "")) => result.context(tr!("Dashboard discovery timed out. Enter its URL in the profile.", "대시보드 주소 검색 시간이 초과되었습니다. 프로필에 URL을 입력하세요."))??,
            _ = stop.changed() => tr_bail!("Connection cancelled.", "연결을 취소했습니다."),
        }
    } else {
        "https://console-openshift-console.apps.ocp4.example.com/".into()
    };
    if runtime.ssh.is_none() {
        tokio::select! {
            result = rhtlc::probe(runtime.socks_port, &dashboard) => result?,
            _ = stop.changed() => tr_bail!("Connection cancelled.", "연결을 취소했습니다."),
        }
    }
    let browser = find_browser(&config.browser_path)?;
    let requested = if *opened {
        ""
    } else {
        &config.browser_profile_path
    };
    let (profile, temporary) = prepare_browser_profile(requested)?;
    if *stop.borrow() {
        cleanup_browser_profile(app, temporary.then_some(profile), false);
        tr_bail!("The connection is stopping.", "연결을 종료하는 중입니다.");
    }
    let child = match launch_browser(
        &browser.path,
        &profile,
        runtime.socks_port,
        &dashboard,
        &config.browser_language,
        config.insecure_tls,
    ) {
        Ok(child) => child,
        Err(error) => {
            cleanup_browser_profile(app, temporary.then_some(profile), false);
            return Err(error);
        }
    };
    let instance = BrowserInstance {
        child,
        temporary_profile: temporary.then_some(profile),
        keep_profile: config.keep_temporary_profile,
    };
    if let Err(error) = runtime.browser_tx.send(instance) {
        let mut instance = error.0;
        let _ = instance.child.kill().await;
        cleanup_browser_profile(app, instance.temporary_profile, instance.keep_profile);
        tr_bail!("The connection has stopped.", "연결이 종료되었습니다.");
    }
    *opened = true;
    emit_log(
        app,
        tr_format!(
            "Opened dashboard: {}",
            "대시보드를 열었습니다: {}",
            dashboard
        ),
    );
    Ok(())
}

#[tauri::command]
async fn open_external_tool(
    app: AppHandle,
    state: State<'_, AppState>,
    tool: String,
) -> std::result::Result<(), String> {
    open_external_inner(&app, &state, &tool)
        .await
        .map_err(display_error)
}

async fn open_external_inner(app: &AppHandle, state: &AppState, tool: &str) -> Result<()> {
    if !matches!(tool, "terminal" | "vscode") {
        tr_bail!(
            "Unsupported developer tool.",
            "지원하지 않는 개발 도구입니다."
        );
    }
    let runtime = active_runtime(state).await?;
    let mut stop = runtime.stop.subscribe();
    let mut session = runtime.tools.lock().await;
    let root = app.path().app_data_dir()?.join("remote-tools");
    let user = if runtime.ssh.is_some() {
        &runtime.config.ssh_user
    } else {
        &runtime.config.remote_user
    };
    if *stop.borrow() {
        tr_bail!("The connection is stopping.", "연결을 종료하는 중입니다.");
    }
    if session.is_none() {
        *session = Some(tokio::select! {
            result = remote_tools::Session::prepare(&root, runtime.route.clone(), user, &runtime.config.private_key_path, stop.clone()) => result?,
            _ = stop.changed() => tr_bail!("Connection cancelled.", "연결을 취소했습니다."),
        });
    }
    if *stop.borrow() {
        tr_bail!("The connection is stopping.", "연결을 종료하는 중입니다.");
    }
    let session = session.as_ref().unwrap();
    if tool == "terminal" {
        tokio::select! {
            result = session.open_terminal(app, stop.clone()) => result?,
            _ = stop.changed() => tr_bail!("Connection cancelled.", "연결을 취소했습니다."),
        }
    } else {
        tokio::select! {
            result = session.open_vscode(&root, &runtime.config.vscode_path, &runtime.config.remote_folder, app, stop.clone()) => result?,
            _ = stop.changed() => tr_bail!("Connection cancelled.", "연결을 취소했습니다."),
        }
    }
    emit_log(
        app,
        tr_format!(
            "Sent a launch request to {}. Complete SSH sign-in in that application; the connection stays active until Disconnect.",
            "{} 실행을 요청했습니다. 해당 프로그램에서 SSH 로그인을 완료하세요. 연결 종료 전까지 접속을 유지합니다.",
            if tool == "terminal" {
                "Terminal"
            } else {
                "VS Code"
            }
        ),
    );
    Ok(())
}

#[tauri::command]
fn detect_browser(requested: String) -> std::result::Result<DetectedBrowser, String> {
    find_browser(requested.trim()).map_err(display_error)
}

#[tauri::command]
fn detect_tools(requested: String) -> remote_tools::ToolAvailability {
    remote_tools::detect_tools(requested.trim())
}

#[tauri::command]
async fn open_installation_guide(topic: String) -> std::result::Result<(), String> {
    remote_tools::open_help(&topic).await.map_err(display_error)
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
    config.remote_host = config.remote_host.trim().to_string();
    config.remote_user = config.remote_user.trim().to_string();
    config.remote_folder = config.remote_folder.trim().to_string();
    config.vscode_path = config.vscode_path.trim().to_string();
    if !config.remote_folder.is_empty() {
        remote_tools::validate_folder(&config.remote_folder)?;
    }

    if config.connection_mode == "ssh" {
        let parsed_ip: IpAddr = config.ssh_ip.parse().context(tr!(
            "Enter a valid SSH IP address.",
            "올바른 SSH IP 주소를 입력해 주세요."
        ))?;
        if !matches!(parsed_ip, IpAddr::V4(_)) {
            tr_bail!(
                "This version supports IPv4 SSH addresses.",
                "현재 버전은 IPv4 SSH 주소를 사용합니다."
            );
        }
        remote_tools::validate_user(&config.ssh_user)?;
        if config.ssh_port == 0 {
            tr_bail!(
                "SSH port must be between 1 and 65535.",
                "SSH 포트는 1~65535 사이여야 합니다."
            );
        }
    } else if config.connection_mode == "rhtlc" {
        config.rhtlc_url = config.rhtlc_url.trim().to_string();
        rhtlc::validate_url(&config.rhtlc_url)?;
        if !config.remote_host.is_empty() {
            if !remote_tools::valid_target(&config.remote_host) || config.remote_port == 0 {
                tr_bail!(
                    "Enter a valid SSH target host and port.",
                    "올바른 SSH 대상 호스트와 포트를 입력하세요."
                );
            }
            remote_tools::validate_user(&config.remote_user)?;
        }
    } else {
        tr_bail!(
            "Unsupported connection method.",
            "지원하지 않는 접속 방식입니다."
        );
    }
    if !(1024..=65515).contains(&config.first_socks_port) {
        tr_bail!(
            "SOCKS port must be between 1024 and 65515.",
            "SOCKS 포트는 1024~65515 사이여야 합니다."
        );
    }
    browser_locale(&config.browser_language)?;
    if !config.dashboard_url.is_empty() {
        let parsed = Url::parse(&config.dashboard_url).context(tr!(
            "Dashboard URL must start with http:// or https://.",
            "대시보드 URL은 http:// 또는 https://로 시작해야 합니다."
        ))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            tr_bail!(
                "Dashboard URL must start with http:// or https://.",
                "대시보드 URL은 http:// 또는 https://로 시작해야 합니다."
            );
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
    .context(tr!(
        "The SSH server timed out.",
        "SSH 서버 응답 시간이 초과되었습니다."
    ))?
    .context(tr!(
        "Cannot connect to the SSH server. Check the IP address and port.",
        "SSH 서버에 연결할 수 없습니다. IP와 포트를 확인해 주세요."
    ))?;

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
                emit_log(
                    app,
                    tr_format!(
                        "Using SSH private-key authentication: {}",
                        "SSH 개인 키 인증 사용: {}",
                        path.display()
                    ),
                );
                return Ok(session);
            }
            Ok(false) => emit_log(
                app,
                tr!(
                    "SSH private key rejected; trying password authentication.",
                    "SSH 개인 키가 거부되어 비밀번호 인증을 시도합니다."
                ),
            ),
            Err(error) => emit_log(
                app,
                tr_format!(
                    "Cannot use the SSH private key: {error}",
                    "SSH 개인 키를 사용할 수 없습니다: {error}"
                ),
            ),
        }
    }

    if config.password.is_empty() {
        tr_bail!(
            "No usable SSH private key was found. Enter an SSH password.",
            "사용 가능한 SSH 개인 키가 없습니다. SSH 비밀번호를 입력해 주세요."
        );
    }

    let auth = session
        .authenticate_password(&config.ssh_user, &config.password)
        .await
        .context(tr!(
            "Could not complete SSH password authentication.",
            "SSH 비밀번호 인증을 완료할 수 없습니다."
        ))?;
    if !auth.success() {
        tr_bail!(
            "SSH authentication failed. Check your username and password.",
            "SSH 인증에 실패했습니다. 사용자 이름과 비밀번호를 확인해 주세요."
        );
    }
    emit_log(
        app,
        tr!(
            "Used SSH password authentication.",
            "SSH 비밀번호 인증을 사용했습니다."
        ),
    );
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
    let key = load_secret_key(path, secret).context(tr!(
        "Could not read the private-key file.",
        "개인 키 파일을 읽지 못했습니다."
    ))?;
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
        emit_log(
            app,
            tr!(
                "Automatically found the OpenShift console address.",
                "OpenShift 콘솔 주소를 자동으로 찾았습니다."
            ),
        );
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
        emit_log(
            app,
            tr!(
                "Automatically found the OpenShift console route.",
                "OpenShift console Route를 자동으로 찾았습니다."
            ),
        );
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
        emit_log(
            app,
            tr!(
                "Automatically found the RHOAI dashboard route.",
                "RHOAI Dashboard Route를 자동으로 찾았습니다."
            ),
        );
        return Ok(format!("https://{rhoai_host}/"));
    }

    emit_log(
        app,
        tr!(
            "Console address not found; using the default training address.",
            "콘솔 주소를 찾지 못해 현재 실습 기본 주소를 사용합니다."
        ),
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
    tr_bail!(
        "No SOCKS port is available in the range {}–{}.",
        "{}~{} 범위에서 사용할 수 있는 SOCKS 포트를 찾지 못했습니다.",
        first_port,
        first_port + 19
    )
}

async fn run_proxy_loop(
    app: AppHandle,
    listener: TcpListener,
    ssh: Arc<client::Handle<SshClient>>,
    mut browser_rx: mpsc::UnboundedReceiver<BrowserInstance>,
    stop_tx: watch::Sender<bool>,
    mut stop: watch::Receiver<bool>,
) {
    let mut browsers = JoinSet::new();
    let mut clients = JoinSet::new();
    let mut health = tokio::time::interval(Duration::from_secs(1));
    emit_browser_count(&app, browsers.len());

    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    emit_log(&app, tr!("Disconnecting at your request.", "사용자 요청으로 연결을 종료합니다."));
                    break;
                }
            }
            Some(browser) = browser_rx.recv() => {
                spawn_browser_watcher(&mut browsers, app.clone(), browser, stop.clone());
                emit_browser_count(&app, browsers.len());
            }
            finished = browsers.join_next(), if !browsers.is_empty() => {
                if let Some(Err(error)) = finished {
                    emit_log(&app, tr_format!("Browser watcher error: {error}", "브라우저 감시 작업 오류: {error}"));
                }
                emit_browser_count(&app, browsers.len());
            }
            _ = clients.join_next(), if !clients.is_empty() => {},
            _ = health.tick() => { if ssh.is_closed() { break; } }
            accepted = listener.accept() => {
                match accepted {
                    Ok((socket, peer)) => {
                        let ssh = Arc::clone(&ssh);
                        let app = app.clone();
                        clients.spawn(async move {
                            if let Err(error) = proxy_socks_connection(socket, peer, ssh).await {
                                emit_log(&app, tr_format!("Proxy connection error: {error}", "프록시 연결 오류: {error}"));
                            }
                        });
                    }
                    Err(error) => {
                        emit_log(&app, tr_format!("SOCKS accept error: {error}", "SOCKS 연결 수락 오류: {error}"));
                        break;
                    }
                }
            }
        }
    }

    let _ = stop_tx.send(true);
    browser_rx.close();
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    while let Some(result) = browsers.join_next().await {
        if let Err(error) = result {
            emit_log(
                &app,
                tr_format!(
                    "Browser shutdown error: {error}",
                    "브라우저 종료 작업 오류: {error}"
                ),
            );
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

    if let Some(runtime) = app.state::<AppState>().runtime.lock().await.take() {
        if let Some(session) = runtime.tools.lock().await.take() {
            session.shutdown().await;
        }
    }
    emit_status(&app, "stopped", tr!("Not connected", "연결 안 됨"));
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
                Ok(_) => emit_log(&app, tr!("Browser process exited.", "브라우저 창이 닫혔습니다.")),
                Err(error) => emit_log(&app, tr_format!("Browser status error: {error}", "브라우저 상태 확인 오류: {error}")),
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
            tr_format!(
                "Kept temporary browser profile: {}",
                "임시 브라우저 프로필을 보관했습니다: {}",
                path.display()
            ),
        );
    } else if let Err(error) = std::fs::remove_dir_all(&path)
        && path.exists()
    {
        emit_log(
            app,
            tr_format!(
                "Temporary profile cleanup error: {error}",
                "임시 프로필 정리 오류: {error}"
            ),
        );
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
        tr_bail!(
            "Unsupported SOCKS request.",
            "지원되지 않는 SOCKS 요청입니다."
        );
    }
    let mut methods = vec![0u8; greeting[1] as usize];
    socket.read_exact(&mut methods).await?;
    socket.write_all(&[5, 0]).await?;

    let mut request = [0u8; 4];
    socket.read_exact(&mut request).await?;
    if request[0] != 5 || request[1] != 1 {
        socket.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        tr_bail!(
            "Only SOCKS CONNECT is supported.",
            "SOCKS CONNECT 이외의 명령은 지원하지 않습니다."
        );
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
            String::from_utf8(name).context(tr!(
                "The SOCKS hostname is not valid UTF-8.",
                "SOCKS 호스트 이름이 UTF-8이 아닙니다."
            ))?
        }
        4 => {
            let mut octets = [0u8; 16];
            socket.read_exact(&mut octets).await?;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        _ => tr_bail!(
            "Unsupported SOCKS address type.",
            "지원되지 않는 SOCKS 주소 형식입니다."
        ),
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
        tr_bail!(
            "Cannot find the selected browser executable: {}",
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
            anyhow!(tr!(
                "Chrome, Chromium, or Edge was not found. Enter the browser executable path.",
                "Chrome, Chromium 또는 Edge를 찾지 못했습니다. 실행 파일 경로를 직접 입력해 주세요."
            ))
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
        _ => tr_bail!(
            "Choose English or Korean as the browser language.",
            "브라우저 언어는 English 또는 한국어 중에서 선택해 주세요."
        ),
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
    std::fs::create_dir_all(&path).context(tr!(
        "Cannot create the browser profile folder.",
        "브라우저 프로필 폴더를 만들 수 없습니다."
    ))?;
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
    command.spawn().with_context(|| {
        tr_format!(
            "Cannot launch the browser: {}",
            "브라우저를 실행할 수 없습니다: {}",
            executable.display()
        )
    })
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
            open_external_tool,
            detect_browser,
            detect_tools,
            open_installation_guide,
            detect_rhtlc,
            set_app_language,
            stop_connection
        ])
        .build(tauri::generate_context!())
        .expect(tr!(
            "Cannot start Remote OCP.",
            "Remote OCP 앱을 시작할 수 없습니다."
        ));

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
fn detect_rhtlc() -> std::result::Result<(), String> {
    rhtlc::find_engine().map(|_| ()).map_err(display_error)
}

async fn start_rhtlc_connection(
    app: AppHandle,
    state: &AppState,
    mut config: ConnectionConfig,
    mut cancel: watch::Receiver<bool>,
) -> Result<ConnectionStarted> {
    let engine = rhtlc::find_engine()?;
    let scope = rhtlc::identity(&config.rhtlc_url)?;
    let (reservation, port) = bind_socks_listener(config.first_socks_port).await?;
    drop(reservation);
    let mut tunnel = rhtlc::Connection::new(engine, port);
    let route = remote_tools::Route {
        host: config.remote_host.clone(),
        port: config.remote_port,
        socks_port: Some(port),
        scope,
    };
    emit_status(
        &app,
        "connecting",
        tr!("Verifying RHTLC access…", "RHTLC 접속을 확인하는 중…"),
    );
    let preparation = {
        let work = async {
            tunnel.connect(&config.rhtlc_url).await?;
            if !route.host.is_empty() {
                remote_tools::probe_ssh(&route).await
            } else {
                let dashboard = if config.dashboard_url.is_empty() {
                    "https://console-openshift-console.apps.ocp4.example.com/"
                } else {
                    &config.dashboard_url
                };
                rhtlc::probe(port, dashboard).await
            }
        };
        tokio::pin!(work);
        tokio::select! {
            result = &mut work => result,
            _ = cancel.changed() => Err(anyhow!(tr!("Connection cancelled.", "연결을 취소했습니다."))),
        }
    };
    config.rhtlc_url.clear();
    config.password.clear();
    config.private_key_passphrase.clear();
    if let Err(error) = preparation {
        let _ = tunnel.disconnect().await;
        return Err(error);
    }
    let (stop, stopped) = watch::channel(false);
    let (sender, receiver) = mpsc::unbounded_channel();
    *state.runtime.lock().await = Some(Arc::new(RunningTunnel {
        stop: stop.clone(),
        browser_tx: sender,
        config,
        socks_port: port,
        ssh: None,
        route,
        tools: Mutex::new(None),
        browser_opened: Mutex::new(false),
    }));
    if *cancel.borrow() {
        let _ = stop.send(true);
    }
    emit_log(
        &app,
        tr!(
            "RHTLC connected. Choose a tool; the connection stays active until Disconnect.",
            "RHTLC에 연결되었습니다. 사용할 도구를 선택하세요. 연결 종료 전까지 접속을 유지합니다."
        ),
    );
    tauri::async_runtime::spawn(watch_rhtlc(app, tunnel, receiver, stop, stopped));
    Ok(ConnectionStarted { socks_port: port })
}

async fn watch_rhtlc(
    app: AppHandle,
    mut tunnel: rhtlc::Connection,
    mut incoming: mpsc::UnboundedReceiver<BrowserInstance>,
    stop: watch::Sender<bool>,
    mut stopped: watch::Receiver<bool>,
) {
    let mut browsers = JoinSet::new();
    emit_browser_count(&app, 0);
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
            _ = browsers.join_next(), if !browsers.is_empty() => { emit_browser_count(&app, browsers.len()); }
            _ = tunnel.wait() => { failed = true; break; }
        }
    }
    let _ = stop.send(true);
    incoming.close();
    while browsers.join_next().await.is_some() {}
    while let Some(mut browser) = incoming.recv().await {
        let _ = browser.child.kill().await;
        cleanup_browser_profile(&app, browser.temporary_profile, browser.keep_profile);
    }
    if tunnel.disconnect().await.is_err() {
        failed = true;
    }
    if let Some(runtime) = app.state::<AppState>().runtime.lock().await.take() {
        if let Some(session) = runtime.tools.lock().await.take() {
            session.shutdown().await;
        }
    }
    emit_browser_count(&app, 0);
    emit_status(
        &app,
        if failed { "error" } else { "stopped" },
        if failed {
            tr!(
                "The RHTLC connection was lost.",
                "RHTLC 연결이 끊어졌습니다."
            )
        } else {
            tr!("Not connected", "연결 안 됨")
        },
    );
}

#[tauri::command]
fn set_app_language(language: String) -> std::result::Result<(), String> {
    language::set(&language)
}
