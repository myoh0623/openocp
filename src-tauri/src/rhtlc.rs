//! Adapter for the official RHTLC CLI. Never log its output: it may contain tokens.
use super::*;
use std::process::Stdio;

pub(crate) fn validate_url(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(|_| anyhow!("RHTLC 주소를 확인해 주세요."))?;
    if url.scheme() != "rhtlc"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !url
            .query_pairs()
            .any(|(key, value)| key == "token" && !value.is_empty())
        || value.chars().any(char::is_control)
    {
        bail!("Copy RHTLC URL로 복사한 rhtlc:// 주소 전체를 붙여넣어 주세요.");
    }
    Ok(())
}

pub(crate) fn find_cli(requested: &str) -> Result<PathBuf> {
    if !requested.is_empty() {
        let path = expand_home(requested);
        if path.is_file() {
            return Ok(path);
        }
        bail!("선택한 RHTLC 실행 파일을 찾을 수 없습니다.");
    }
    let mut paths = vec![];
    let name = if cfg!(windows) { "rhtlc.exe" } else { "rhtlc" };
    if let Some(value) = env::var_os("PATH") {
        paths.extend(env::split_paths(&value).map(|p| p.join(name)));
    }
    if cfg!(target_os = "macos") {
        paths.extend([
            PathBuf::from("/opt/homebrew/bin/rhtlc"),
            PathBuf::from("/usr/local/bin/rhtlc"),
        ]);
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x86_64"
        };
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = home_directory() {
            roots.push(home.join("Applications"));
        }
        for root in roots {
            paths.push(root.join(format!(
                "RHTLC-GUI.app/Contents/Resources/cli/rhtlc-macos-{arch}"
            )));
        }
    } else if cfg!(windows) {
        for key in ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"] {
            if let Some(root) = env::var_os(key) {
                paths.push(PathBuf::from(&root).join("Programs/RHTLC/rhtlc.exe"));
                paths.push(PathBuf::from(root).join("RHTLC/rhtlc.exe"));
            }
        }
    }
    paths.into_iter().find(|p| p.is_file()).ok_or_else(|| anyhow!(
        "RHTLC가 설치되어 있지 않습니다. 공식 RHTLC 클라이언트를 설치하거나 고급 설정에서 실행 파일 경로를 지정해 주세요."
    ))
}

fn command(path: &Path) -> Command {
    let mut cmd = Command::new(path);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd
}

pub(crate) struct Connection {
    cli: PathBuf,
    sku: String,
    pub(crate) port: u16,
}

impl Connection {
    pub(crate) fn new(cli: PathBuf, port: u16) -> Self {
        Self {
            cli,
            port,
            sku: format!("remote-ocp-{}", Uuid::new_v4()),
        }
    }

    pub(crate) async fn connect(&mut self, url: &str) -> Result<()> {
        // stdin keeps the token out of process command lines and shell history.
        let mut child = command(&self.cli)
            .args([
                "connect",
                "--sku",
                &self.sku,
                "--port",
                &self.port.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("RHTLC 실행 파일을 시작하지 못했습니다.")?;
        let mut stdin = child
            .stdin
            .take()
            .context("RHTLC 입력을 준비하지 못했습니다.")?;
        stdin.write_all(url.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        drop(stdin);
        let output = timeout(Duration::from_secs(45), child.wait_with_output())
            .await
            .context(
                "RHTLC 연결 시간이 초과되었습니다. 실습 실행 상태와 주소를 확인해 주세요.",
            )??;
        if !output.status.success() {
            bail!(
                "RHTLC 연결에 실패했습니다. 실습이 실행 중인지 확인하고 새 주소를 복사해 주세요."
            );
        }
        // The CLI can allocate a different port if the requested one is occupied.
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(port) = Regex::new(r"127\.0\.0\.1:(\d+)")?
            .captures_iter(&stdout)
            .last()
            .and_then(|m| m[1].parse::<u16>().ok())
        {
            self.port = port;
        }
        Ok(())
    }

    pub(crate) async fn disconnect(&self) -> Result<()> {
        let status = timeout(
            Duration::from_secs(15),
            command(&self.cli)
                .args(["disconnect", "--sku", &self.sku])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(),
        )
        .await
        .context("RHTLC 종료 시간이 초과되었습니다.")??;
        if !status.success() {
            bail!("RHTLC 연결을 종료하지 못했습니다.");
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
    fn validates_links_without_exposing_tokens() {
        assert!(validate_url("rhtlc://lab.example.com/lab?token=secret&sku=DO180").is_ok());
        for value in [
            "https://lab.example.com/lab?token=secret",
            "rhtlc://lab/lab",
            "rhtlc://u:p@lab/lab?token=secret",
            "rhtlc://lab/lab?token=",
            "rhtlc://lab/lab?token=secret\n",
        ] {
            let error = validate_url(value).unwrap_err().to_string();
            assert!(!error.contains("secret"));
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn cli_uses_stdin_allocated_port_and_scoped_disconnect() {
        use std::os::unix::fs::PermissionsExt;
        let directory = env::temp_dir().join(format!("remote-ocp-cli-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let cli = directory.join("rhtlc");
        std::fs::write(
            &cli,
            r#"#!/bin/sh
case "$3" in remote-ocp-*) ;; *) exit 9;; esac
if [ "$1" = "disconnect" ]; then
  [ "$#" = 3 ] && [ "$2" = "--sku" ] || exit 8
  exit 0
fi
[ "$#" = 5 ] && [ "$1" = "connect" ] && [ "$4" = "--port" ] || exit 7
IFS= read -r link
case "$link" in
  *bad-token*) echo "error: bad-token" >&2; exit 1;;
  'rhtlc://lab.example.com/lab?token=test-only') echo 'SOCKS5 proxy: 127.0.0.1:11001';;
  *) exit 6;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut connection = Connection::new(cli, 1080);
        connection
            .connect("rhtlc://lab.example.com/lab?token=test-only")
            .await
            .unwrap();
        assert_eq!(connection.port, 11001);
        let error = connection
            .connect("rhtlc://lab.example.com/lab?token=bad-token")
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("bad-token"));
        connection.disconnect().await.unwrap();
        std::fs::remove_dir_all(directory).unwrap();
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
