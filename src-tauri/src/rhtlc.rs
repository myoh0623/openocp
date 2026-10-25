//! Own the bundled RHTLC transport directly; no installed CLI or shared daemon.
use super::*;
use std::process::Stdio;

/// Match RHTLC 6.0.2: use the lab's WebSocket endpoint and discard the portal
/// query (including its token) before launching the transport. Never log input.
fn endpoint(value: &str) -> Result<Url> {
    let invalid = || anyhow!("Copy RHTLC URL로 복사한 rhtlc:// 주소 전체를 붙여넣어 주세요.");
    let address = value.strip_prefix("rhtlc://").ok_or_else(invalid)?;
    let address = address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("wss://"))
        .unwrap_or(address);
    let mut url = Url::parse(&format!("wss://{address}")).map_err(|_| invalid())?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !url
            .query_pairs()
            .any(|(key, value)| key == "token" && !value.is_empty())
        || value.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    url.set_query(None);
    if url.path() == "/" || url.path().is_empty() {
        url.set_path(
            if url.host_str().unwrap().ends_with(".labs.ole.redhat.com") {
                "/lab"
            } else {
                "/socks5"
            },
        );
    }
    Ok(url)
}

pub(crate) fn validate_url(value: &str) -> Result<()> {
    endpoint(value).map(|_| ())
}

pub(crate) fn find_engine() -> Result<PathBuf> {
    // Tauri copies the sidecar beside the executable in both dev and bundles.
    // Never search PATH or the separately installed official client's folders.
    let name = if cfg!(windows) {
        "rhtlc-wstunnel.exe"
    } else {
        "rhtlc-wstunnel"
    };
    let path = env::current_exe()?
        .parent()
        .context("앱 위치를 확인하지 못했습니다.")?
        .join(name);
    if !path.is_file() {
        bail!("앱에 포함된 RHTLC 엔진을 찾을 수 없습니다. Remote OCP를 다시 설치해 주세요.");
    }
    Ok(path)
}

fn command(path: &Path) -> Command {
    let mut cmd = Command::new(path);
    cmd.kill_on_drop(true);
    // Some terminals set NO_COLOR=1; this engine expects a boolean string.
    cmd.env("NO_COLOR", "true");
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd
}

pub(crate) struct Connection {
    engine: PathBuf,
    child: Option<Child>,
    pub(crate) port: u16,
}

impl Connection {
    pub(crate) fn new(engine: PathBuf, port: u16) -> Self {
        Self {
            engine,
            child: None,
            port,
        }
    }

    fn launch_command(&self, url: &str) -> Result<Command> {
        let endpoint = endpoint(url)?;
        let mut cmd = command(&self.engine);
        cmd.args([
            "--log-lvl",
            "OFF",
            "--nb-worker-threads",
            "2",
            "client",
            "--tls-verify-certificate",
            "-L",
            &format!("socks5://127.0.0.1:{}", self.port),
            endpoint.as_str(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        Ok(cmd)
    }

    pub(crate) async fn connect(&mut self, url: &str) -> Result<()> {
        if self.child.is_some() {
            bail!("RHTLC 연결이 이미 실행 중입니다.");
        }
        self.child = Some(
            self.launch_command(url)?
                .spawn()
                .context("내장 RHTLC 엔진을 시작하지 못했습니다.")?,
        );
        timeout(Duration::from_secs(10), async {
            loop {
                if self.child.as_mut().unwrap().try_wait()?.is_some() {
                    bail!("내장 RHTLC 엔진이 종료되었습니다. 프록시 포트를 확인해 주세요.");
                }
                if TcpStream::connect((Ipv4Addr::LOCALHOST, self.port))
                    .await
                    .is_ok()
                {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .context("내장 RHTLC 엔진 시작 시간이 초과되었습니다.")?
    }

    pub(crate) async fn wait(&mut self) -> Result<()> {
        self.child
            .as_mut()
            .context("RHTLC 엔진이 실행 중이 아닙니다.")?
            .wait()
            .await?;
        Ok(())
    }

    pub(crate) async fn disconnect(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            if child.try_wait()?.is_none() {
                child
                    .kill()
                    .await
                    .context("내장 RHTLC 엔진을 종료하지 못했습니다.")?;
            }
        }
        Ok(())
    }
}

/// Verify that traffic can actually reach the requested dashboard through SOCKS.
/// DNS resolution remains at the lab, never on the user's VPN/local resolver.
pub(crate) async fn probe(port: u16, dashboard: &str) -> Result<()> {
    let url = Url::parse(dashboard)?;
    let host = url.host_str().context("대시보드 호스트가 없습니다.")?;
    let target_port = url
        .port_or_known_default()
        .context("대시보드 포트를 확인해 주세요.")?;
    if host.len() > 255 {
        bail!("대시보드 호스트 이름이 너무 깁니다.");
    }
    timeout(Duration::from_secs(15), async {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
        stream.write_all(&[5, 1, 0]).await?;
        let mut response = [0; 2];
        stream.read_exact(&mut response).await?;
        if response != [5, 0] { bail!("SOCKS5 인증 응답이 올바르지 않습니다."); }
        let mut request = vec![5, 1, 0, 3, host.len() as u8];
        request.extend_from_slice(host.as_bytes());
        request.extend_from_slice(&target_port.to_be_bytes());
        stream.write_all(&request).await?;
        let mut response = [0; 4];
        stream.read_exact(&mut response).await?;
        if response[0] != 5 || response[1] != 0 { bail!("대시보드에 연결할 수 없습니다."); }
        Ok::<(), anyhow::Error>(())
    }).await.context("실습 대시보드 응답 시간이 초과되었습니다.")?
      .context("RHTLC 프록시를 통해 대시보드에 접속하지 못했습니다. 새 RHTLC 주소와 대시보드 URL을 확인해 주세요.")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_portal_links_without_exposing_tokens() {
        for value in [
            "rhtlc://lab.labs.ole.redhat.com/lab?token=secret&sku=DO180",
            "rhtlc://https://lab.labs.ole.redhat.com/?token=secret",
            "rhtlc://wss://lab.labs.ole.redhat.com/lab?token=secret",
        ] {
            assert_eq!(
                endpoint(value).unwrap().as_str(),
                "wss://lab.labs.ole.redhat.com/lab"
            );
        }
        assert_eq!(
            endpoint("rhtlc://lab.example.com:8443/custom?token=secret")
                .unwrap()
                .as_str(),
            "wss://lab.example.com:8443/custom"
        );
        for value in [
            "https://lab.example.com/lab?token=secret",
            "rhtlc://lab/lab",
            "rhtlc://u:p@lab/lab?token=secret",
            "rhtlc://lab/lab?token=",
            "rhtlc://lab/lab?token=secret\n",
            "rhtlc://lab/lab?token=secret#fragment",
        ] {
            assert!(
                !validate_url(value)
                    .unwrap_err()
                    .to_string()
                    .contains("secret")
            );
        }
        let connection = Connection::new("engine".into(), 1080);
        let cmd = connection
            .launch_command("rhtlc://lab/lab?token=secret")
            .unwrap();
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|s| s.to_str().unwrap())
            .collect();
        assert!(!format!("{args:?}").contains("secret"));
        assert!(args.contains(&"--tls-verify-certificate"));
        assert!(args.contains(&"socks5://127.0.0.1:1080"));
        assert_eq!(args.last(), Some(&"wss://lab/lab"));
    }

    #[tokio::test]
    async fn bundled_engine_starts_stops_and_drops_without_a_cli() {
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "binaries/rhtlc-wstunnel-{}{suffix}",
            env!("TAURI_ENV_TARGET_TRIPLE")
        ));
        let version = command(&engine).arg("--version").output().await.unwrap();
        assert!(version.status.success());
        assert_eq!(
            String::from_utf8_lossy(&version.stdout).trim(),
            "rhtlc-wstunnel 6.0.1"
        );
        for explicit_stop in [true, false] {
            let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let port = reservation.local_addr().unwrap().port();
            drop(reservation);
            let mut connection = Connection::new(engine.clone(), port);
            // No external traffic: the engine opens upstream only on a SOCKS request.
            connection
                .connect("rhtlc://127.0.0.1:9/lab?token=test-only")
                .await
                .unwrap();
            assert!(
                connection
                    .child
                    .as_mut()
                    .unwrap()
                    .try_wait()
                    .unwrap()
                    .is_none()
            );
            if explicit_stop {
                connection.disconnect().await.unwrap();
            }
            drop(connection);
            timeout(Duration::from_secs(5), async {
                while TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                    .await
                    .is_ok()
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("owned proxy should close on disconnect or cancellation/drop");
        }
        let mut missing = Connection::new(PathBuf::from("missing-rhtlc-test-engine"), 1080);
        assert!(
            !missing
                .connect("rhtlc://lab/lab?token=secret")
                .await
                .unwrap_err()
                .to_string()
                .contains("secret")
        );
        missing.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn sends_remote_dns_name_and_rejects_proxy_failure() {
        for code in [0, 5] {
            let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let port = server.local_addr().unwrap().port();
            let task = tokio::spawn(async move {
                let (mut socket, _) = server.accept().await.unwrap();
                let mut greeting = [0; 3];
                socket.read_exact(&mut greeting).await.unwrap();
                assert_eq!(greeting, [5, 1, 0]);
                socket.write_all(&[5, 0]).await.unwrap();
                let mut header = [0; 5];
                socket.read_exact(&mut header).await.unwrap();
                assert_eq!(&header[..4], &[5, 1, 0, 3]);
                let mut host = vec![0; header[4] as usize];
                socket.read_exact(&mut host).await.unwrap();
                assert_eq!(host, b"console.apps.example.com");
                assert_eq!(socket.read_u16().await.unwrap(), 443);
                socket
                    .write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0])
                    .await
                    .unwrap();
            });
            assert_eq!(
                probe(port, "https://console.apps.example.com/")
                    .await
                    .is_ok(),
                code == 0
            );
            task.await.unwrap();
        }
    }
}
