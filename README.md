# Remote OCP 2.1

Remote OCP는 IP·SSH 또는 RHTLC 주소로 연결해 OpenShift 또는 RHOAI 대시보드를 여는 macOS/Windows 데스크톱 앱입니다.

하나의 **Rust + Tauri** 코드베이스에서 macOS 앱과 Windows 실행 파일을 함께 만듭니다.

## 주요 기능

- IP·SSH / RHTLC 주소 접속 방식 선택
- Copy RHTLC URL 주소로 공식 RHTLC 프록시 연결 후 독립 Chrome/Edge 자동 실행
- SSH IP, 사용자, 포트와 대시보드 URL 입력
- 대시보드 URL을 비워두면 원격 서버에서 OpenShift/RHOAI Route 자동 탐색
- 여러 접속 정보를 프로필로 저장하고 다음 실행 때 다시 사용
- 비밀번호와 개인 키 암호는 저장하지 않음
- 내장 Rust SSH 클라이언트와 SOCKS5 프록시 사용
- Google Chrome, Chromium 또는 Microsoft Edge 자동 탐색 및 실행 경로 표시
- 프로필마다 English 또는 한국어 브라우저 언어 저장(기본값 English)
- 같은 터널에서 로그인 정보가 분리된 독립 브라우저를 여러 개 실행
- 마지막 브라우저가 닫히면 SSH 연결도 자동 종료
- macOS와 Windows에서 같은 화면과 기능 사용

## RHTLC 주소로 연결 (VPN 불필요)

1. [공식 설치 안내](https://redhattraining.github.io/RHTLC_Docs/)에 따라 RHTLC를 설치합니다. macOS는 Homebrew CLI 또는 RHTLC-GUI에 포함된 CLI, Windows는 공식 설치 패키지의 `rhtlc.exe`를 사용합니다. 현재 통합은 RHTLC 6.0.2를 기준으로 검증합니다.
2. Red Hat 실습 페이지에서 **Copy RHTLC URL**을 누릅니다.
3. Remote OCP에서 연결 방법을 **RHTLC 주소 · VPN 없이 접속**으로 선택하고 주소를 붙여넣습니다.
4. 대시보드 URL을 입력하거나 비워둡니다. 이 방식에서 빈 주소는 기본 실습 OpenShift 콘솔(`https://console-openshift-console.apps.ocp4.example.com/`)을 사용합니다. RHOAI 또는 다른 클러스터는 주소를 직접 입력합니다.
5. **대시보드 열기**를 누르면 프록시 경유 접속을 확인하고 Chrome/Chromium/Edge를 자동으로 엽니다. Firefox의 프로필이나 시스템 VPN·프록시 설정을 직접 바꿀 필요가 없습니다.
6. 마지막 독립 브라우저를 닫거나 연결을 종료하면 Remote OCP가 만든 RHTLC 연결만 정리합니다.

고급 설정의 **RHTLC 실행 파일 → 경로 확인**에서 설치 경로를 확인할 수 있습니다. 자동으로 찾지 못하면 CLI 실행 파일 경로를 지정합니다. 브라우저 프로필 폴더는 비워두면 임시 폴더를 자동 생성합니다.

접속 방식과 실행 파일 경로는 프로필에 저장하지만, 토큰이 포함된 RHTLC 주소는 저장하지 않습니다. 다시 연결할 때 새 주소를 붙여넣으세요. 주소는 명령행 인수가 아닌 표준 입력으로 공식 CLI에 전달하며 CLI 원문 출력도 앱 로그에 남기지 않습니다. 공식 RHTLC 자체는 연결 관리를 위해 자체 설정과 SSH 설정 항목을 생성할 수 있습니다.

## IP·SSH로 연결

1. 운영체제에 맞는 Remote OCP 앱을 실행합니다.
2. SSH IP와 사용자 이름을 입력합니다.
3. 비밀번호 인증이면 SSH 비밀번호를 입력합니다. 개인 키가 있으면 비밀번호를 비워둘 수 있습니다.
4. 대시보드 URL은 필요할 때만 입력합니다. 비워두면 자동으로 탐색합니다.
5. **대시보드 열기**를 누릅니다.
6. 다른 계정으로 동시에 로그인하려면 **＋ 독립 브라우저**를 누릅니다.
7. 열린 브라우저를 모두 닫으면 터널도 종료됩니다.

프로필에는 SSH 비밀번호와 개인 키 암호가 포함되지 않습니다.

각 독립 브라우저는 별도의 임시 프로필을 사용하므로 쿠키와 로그인 세션을 공유하지 않습니다. 예를 들어 한 창에는 `admin`, 다른 창에는 `developer`로 로그인할 수 있습니다. 브라우저 언어 설정은 새로 여는 독립 브라우저에도 동일하게 적용됩니다.

Chrome 계열 브라우저는 일반적인 설치 위치와 시스템 경로에서 자동으로 찾습니다. 고급 설정의 **경로 확인**에서 실제로 선택된 브라우저와 실행 파일 위치를 확인할 수 있습니다. 자동 탐색이 되지 않으면 같은 입력란에 실행 파일 경로를 직접 입력합니다.

## 빌드 결과 받기

GitHub 저장소의 **Actions → Build macOS and Windows**에서 실행 결과를 선택하면 다음 파일을 받을 수 있습니다.

- `Remote-OCP-macOS`: macOS 앱/DMG
- `Remote-OCP-Windows`: Windows 설치용 EXE/MSI

`main` 브랜치에 변경 사항이 올라갈 때마다 두 운영체제용 빌드가 자동으로 생성됩니다.

## 로컬 빌드

필요한 항목:

- Node.js 20 이상
- Rust stable
- macOS에서 macOS 앱을 만들 때 Xcode Command Line Tools
- Windows에서 Windows 앱을 만들 때 Microsoft C++ Build Tools와 WebView2

```bash
npm install
npm run tauri build
```

이 저장소에서 사용하는 프로젝트 전용 Rust 도구가 `.tools`에 준비되어 있다면 다음 스크립트도 사용할 수 있습니다.

```bash
./build-desktop.sh
```

빌드 결과는 `src-tauri/target/release/bundle` 아래에 생성됩니다.

## 프로젝트 구성

- `src/`: 두 운영체제가 공유하는 화면과 프로필 관리
- `src-tauri/src/`: Rust SSH, SOCKS5, URL 탐색 및 브라우저 제어
- `src-tauri/src/rhtlc.rs`: 공식 RHTLC CLI 탐색·연결·종료와 프록시 통신 확인
- `src-tauri/icons/`: macOS/Windows 앱 아이콘
- `.github/workflows/`: macOS/Windows 자동 빌드

실습용 `*.example.com` 주소는 브라우저 인증서 오류를 자동으로 허용합니다. 다른 주소는 고급 설정에서 직접 선택할 수 있습니다.
