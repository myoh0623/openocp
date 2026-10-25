# Remote OCP 2.2

[English](README.md) | [한국어](README.ko.md)

## 다운로드

**[GitHub Actions에서 설치 파일 다운로드](https://github.com/myoh0623/openocp/actions/workflows/build-desktop.yml?query=branch%3Amain)**

1. GitHub에 로그인한 뒤 위 링크를 엽니다.
2. 가장 최근에 성공한 **main** 빌드를 선택하고, 아래의 **Artifacts** 항목으로 이동합니다.
3. 운영체제에 맞는 파일을 내려받아 ZIP 압축을 풀고 설치 파일을 실행합니다.

| 운영체제 | 다운로드 항목 | 설치 파일 |
| --- | --- | --- |
| macOS (Apple Silicon 및 Intel) | `Remote-OCP-macOS` | `.dmg` |
| Windows x64 | `Remote-OCP-Windows` | `.exe` 또는 `.msi` |

빌드 파일은 14일 동안 보관됩니다. 이전 다운로드가 만료되었다면 더 최근에 성공한 빌드를 선택하세요.

## 소개

Remote OCP는 IP/SSH 연결 또는 Red Hat Training Lab Connector(`rhtlc://`) 주소를 통해 OpenShift/RHOAI 대시보드, SSH 터미널, VS Code Remote - SSH 세션을 여는 데스크톱 앱입니다. 하나의 Rust + Tauri 코드로 macOS와 Windows 앱을 빌드합니다.

## 앱 언어

사이드바의 **App language / 앱 언어**에서 **English**와 **한국어**를 선택할 수 있습니다. 기본 언어는 영어이며, 선택한 언어는 다음 실행에도 유지됩니다. 화면, 입력값 검증, 새 연결 메시지에 적용되며 연결 프로필별 브라우저 언어와는 별개입니다.

## 주요 기능

- IP/SSH 또는 RHTLC URL 연결 방식 선택
- VPN이나 별도의 RHTLC 클라이언트 없이 실습 환경에 연결
- SOCKS5 프록시와 독립된 브라우저 프로필을 적용하여 Chrome, Chromium, Edge 실행
- 하나의 연결에서 여러 독립 브라우저 세션 실행: 관리자와 개발자 계정 등을 별도로 사용
- 연결 프로필과 브라우저 언어 저장: 기본 브라우저 언어는 영어
- 비밀번호, 개인 키 암호, 토큰이 포함된 RHTLC URL은 프로필에 저장하지 않음
- 대시보드 URL이 비어 있으면 SSH를 통해 OpenShift/RHOAI 경로 자동 탐색
- 같은 실습 연결로 기본 터미널 또는 VS Code 실행
- 브라우저를 닫아도 연결 유지: 연결 종료 또는 앱 종료 시 터널과 전달 중인 SSH 연결 종료

## RHTLC URL로 연결

1. Red Hat 교육 포털에서 실습 환경을 시작하고 **Copy RHTLC URL**을 선택합니다.
2. Remote OCP에서 **RHTLC 주소** 연결 방식을 선택하고 전체 주소를 붙여넣습니다.
3. 대시보드 URL을 입력합니다. 비워 두면 기본 실습 콘솔인 `https://console-openshift-console.apps.ocp4.example.com/`을 사용합니다. RHOAI나 다른 클러스터라면 URL을 직접 입력하세요.
4. 터미널이나 VS Code를 사용하려면 **터미널 · VS Code**에서 실습 환경의 **실습 SSH 호스트**, 사용자, 포트를 입력합니다. 실습에서 제공하는 호스트(예: `workstation.lab.example.com`)를 사용하세요. 대시보드만 이용할 경우 호스트를 비워 둘 수 있습니다.
5. **연결**을 선택합니다. SSH 호스트가 있으면 SSH 접근을 확인하고, 없으면 대시보드 연결을 확인합니다.
6. **대시보드**, **터미널**, **VS Code** 중 원하는 도구를 선택합니다. 대시보드를 누를 때마다 독립된 브라우저 세션이 열립니다. **연결 종료**를 누르면 앱이 실행한 브라우저, 터널, 전달 중인 SSH 연결이 종료됩니다.

설치 파일에는 작은 `rhtlc-wstunnel` 전송 엔진이 포함되어 있습니다. 공식 Python CLI/GUI, Firefox, VPN을 별도로 설치할 필요가 없습니다. 대시보드에는 Chrome, Chromium 또는 Edge가 필요하며, 터미널과 VS Code는 브라우저 없이 사용할 수 있습니다. 시스템 프록시 설정, 기존 브라우저 프로필, 공식 RHTLC/SSH 설정 파일은 변경하지 않습니다. 브라우저 프로필 폴더를 비워 두면 임시 프로필을 자동으로 만듭니다.

RHTLC URL은 현재 연결에만 사용됩니다. 다시 연결할 때는 새 URL을 붙여넣으세요. 공식 RHTLC 6.0.2 클라이언트와 같은 방식으로 실습 주소를 `wss://`로 변환하고, 전송 엔진을 실행하기 전에 토큰을 포함한 포털 쿼리를 제거합니다. 토큰은 프로세스 실행 인자, 설정 파일, 로그에 남기지 않습니다. 공개 게이트웨이의 TLS 인증서를 검증하며, 내부 실습 호스트 이름은 SOCKS 터널을 통해 해석합니다.

이 통합은 대시보드와 외부 도구를 통한 SSH 접근을 지원합니다. VPN, RDP, SFTP 인터페이스는 포함하지 않습니다. 해당 사용법은 [공식 RHTLC 문서](https://redhattraining.github.io/RHTLC_Docs/)를 참고하세요.

## IP/SSH로 연결

1. **IP/SSH**를 선택하고 SSH IP 주소와 사용자를 입력합니다.
2. 비밀번호를 입력하거나 SSH 개인 키를 사용합니다. 개인 키 암호는 고급 설정에서 입력할 수 있습니다.
3. 대시보드 URL을 입력합니다. 비워 두면 원격 환경에서 경로를 탐색합니다.
4. **연결**을 누른 뒤 **대시보드**, **터미널**, **VS Code** 중 원하는 도구를 선택합니다.

추가로 여는 브라우저마다 별도의 임시 프로필과 독립된 쿠키를 사용합니다. 브라우저 언어는 새 세션에 적용됩니다. 고급 설정에서 자동으로 찾은 브라우저를 확인하거나 실행 파일을 직접 지정할 수 있습니다. SSH 방식은 지정한 IP에 네트워크로 접근할 수 있어야 하며, 환경에 따라 VPN이 필요할 수 있습니다.

## 터미널과 VS Code

**도구 설치 상태** 패널은 연결 전에 OpenSSH, VS Code, Remote - SSH 설치 여부를 확인합니다. 설치 안내 버튼은 기본 브라우저에서 공식 안내 페이지를 엽니다. 필요한 도구를 설치하거나 VS Code 경로를 변경한 뒤 **다시 확인**을 누르세요. 도구 실행 시에도 다시 검사합니다. 확장 프로그램은 `VSCODE_EXTENSIONS`가 설정되어 있으면 해당 경로에서, 그렇지 않으면 기본 확장 폴더에서 확인하며 제거된 것으로 표시된 확장은 제외합니다.

먼저 연결한 뒤 도구를 선택하세요. IP/SSH 방식에서는 설정한 SSH IP, 포트, 사용자를 사용합니다. RHTLC 방식에서는 내장 SOCKS 전송 엔진을 통해 설정한 실습 호스트에 접근하고, 실습 환경 내부에서 호스트 이름을 해석합니다. 로컬 SSH 중계는 `127.0.0.1`에서만 수신하며 실행한 도구들이 함께 사용합니다.

- **터미널**: macOS의 Terminal 또는 Windows Terminal을 엽니다. Windows Terminal이 없으면 별도의 PowerShell 창을 엽니다. Windows에는 선택적 기능인 **OpenSSH 클라이언트**가 필요합니다.
- **VS Code**: 안정 버전의 VS Code와 Microsoft의 **Remote - SSH** 확장이 필요합니다. Remote OCP 전용 창에서 실행하며, 설치된 확장은 공유하고 사용자 설정은 분리합니다. 고급 설정에서 VS Code 앱 또는 실행 파일을 지정할 수 있습니다. Windows에서는 `code.cmd` 대신 `Code.exe`를 선택하세요. 포터블 버전은 독립된 연결 프로필 설정을 무시하므로 실행을 차단하고 안내 메시지를 표시합니다. 일반 설치 버전을 사용하세요.
- **VS Code 원격 폴더**: 선택 사항입니다. `/home/student/project`와 같은 Linux 절대 경로를 입력하세요. 비워 두면 폴더를 열지 않고 연결합니다. 원격 환경은 VS Code Server 설치 권한과 네트워크 접근을 포함한 [VS Code Remote - SSH 요구 사항](https://code.visualstudio.com/docs/remote/ssh)을 충족해야 합니다.
- SSH 인증은 열린 도구에서 완료합니다. Remote OCP에 입력한 비밀번호와 개인 키 암호는 외부 도구로 전달하지 않습니다. 필요한 경우 고급 설정에서 개인 키 경로를 지정하세요. 암호화된 키는 외부 도구에서 잠금을 해제합니다.

Remote OCP는 세션마다 전용 SSH 설정 파일을 만들고, 앱 데이터 폴더의 별도 known-hosts 파일을 사용합니다. OpenSSH는 알 수 없는 호스트 키를 수락하기 전에 확인을 요청하며 변경된 키는 거부합니다. 호스트 식별은 로컬 포트가 달라져도 유지되고, 실습 게이트웨이와 대상별로 구분됩니다. 기존 `~/.ssh/config`는 변경하거나 생성된 설정에서 읽어 들이지 않습니다. VS Code 전용 프로필은 macOS 소켓 경로 길이 제한에 맞춘 짧은 경로인 `~/.remote-ocp/code-<app-id-hash>`에 저장합니다. 프로필의 `User/settings.json`은 주석과 마지막 쉼표를 포함한 VS Code JSONC를 지원합니다. 연결 옵션을 갱신할 때 주석과 다른 설정을 보존하며, 잘못된 설정 파일은 변경하지 않고 오류를 알립니다.

도구 실행 실패는 닫을 수 있는 오류 안내와 활동 로그에 표시됩니다. 실행 직후 오류 코드로 종료하면 실행 요청 성공으로 표시하기 전에 감지하며, 이후 발생한 실패도 연결이 유지되는 동안 알립니다. 실행 요청 성공이 SSH 인증 완료를 뜻하지는 않습니다.

브라우저, 터미널, 편집기를 닫아도 연결은 **종료되지 않습니다**. 사용을 마치면 **연결 종료**를 선택하거나 Remote OCP를 종료하세요. 연결 종료는 앱이 소유한 중계와 활성 SSH 연결을 종료하고 세션 실행 파일을 제거합니다. 저장하지 않은 작업을 잃지 않도록 터미널과 VS Code 창은 그대로 둡니다. 다시 연결할 때는 Remote OCP의 도구 버튼을 사용하세요. 기존 창의 연결 대상은 이전 세션에 속합니다.

## 로컬 빌드

필수 도구: Node.js 22 이상, Rust 안정 버전, 운영체제별 Tauri 빌드 요구 사항(macOS는 Xcode Command Line Tools, Windows는 Microsoft C++ Build Tools와 WebView2).

두 운영체제 모두 PR과 `main` 푸시 시 자동 빌드합니다. Windows에서는 로컬 방화벽이나 보안 정책에 따라 포함된 터널 실행 파일에 대한 안내가 표시될 수 있습니다.

```bash
npm ci
npm run tauri build
```

Tauri 개발·빌드 단계에서 `npm run prepare:rhtlc`를 자동 실행합니다. 대상 플랫폼에 맞는 고정 버전 **rhtlc-wstunnel 6.0.1**을 내려받아 코드에 지정된 SHA-256 체크섬을 검증하고 Tauri 사이드카로 준비합니다. macOS Universal 빌드는 검증된 ARM64와 x64 실행 파일을 `lipo`로 합칩니다. 다운로드는 빌드할 때만 수행하며, 설치된 앱은 다른 클라이언트를 찾거나 내려받지 않고 포함된 실행 파일을 사용합니다.

```bash
# macOS Universal
npm run tauri build -- --target universal-apple-darwin

# Cargo를 직접 실행하기 전에 현재 플랫폼의 전송 엔진 준비
npm run prepare:rhtlc
cargo test --manifest-path src-tauri/Cargo.toml
```

프로젝트의 `.tools`에 Rust 도구가 있으면 `./build-desktop.sh`가 이를 사용합니다. 결과물은 `src-tauri/target/<target>/release/bundle`에 생성되며, 기본 대상 빌드는 `src-tauri/target/release/bundle`에 생성됩니다.

전송 엔진은 설치 파일 압축 전 기준으로 macOS ARM64에서 약 9.6 MiB, Windows x64에서 약 9.9 MiB를 추가합니다. macOS Universal에는 두 아키텍처가 모두 포함됩니다. 하나의 전송 프로세스를 모든 브라우저 세션이 공유하고 작업 스레드는 두 개를 사용합니다. 실제 메모리 사용량은 트래픽과 브라우저 세션에 따라 달라집니다.

소스와 재배포 고지는 앱에도 포함되는 [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt)에 있습니다. 실행 파일 해시와 다운로드 URL은 [scripts/prepare-rhtlc.mjs](scripts/prepare-rhtlc.mjs)에 고정되어 있습니다. Remote OCP는 독립 프로젝트입니다.

## 프로젝트 구조

- `src/`: 공통 UI와 연결 프로필
- `src-tauri/src/`: Rust SSH, SOCKS5, 경로 탐색, 브라우저 수명 관리
- `src-tauri/src/remote_tools.rs`: 로컬 SSH 전달, 전용 OpenSSH 설정, 기본 도구 실행
- `src-tauri/src/rhtlc.rs`: 내장 전송 엔진 관리, URL 처리, SOCKS 연결 확인
- `scripts/prepare-rhtlc.mjs`: macOS·Windows 빌드용 전송 엔진 다운로드 및 검증
- `.github/workflows/`: 데스크톱 빌드와 연결 테스트

`*.example.com` 아래의 실습 대시보드 주소는 브라우저 인증서 오류를 자동으로 허용합니다. 다른 주소에서는 고급 설정에서 명시적으로 허용해야 합니다. 이 브라우저 설정은 공개 RHTLC 게이트웨이의 TLS 인증서 검증을 끄지 않습니다.

## 라이선스

Remote OCP는 [MIT 라이선스](LICENSE)를 따릅니다. 외부 의존성은 각각의 라이선스를 유지합니다. 포함된 `rhtlc-wstunnel` 엔진은 BSD-3-Clause로 배포하며 원본 저작권, 라이선스 조건, 면책 조항은 [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt)에 보존되어 있습니다. 두 파일 모두 앱에 포함됩니다.
