# Remote OCP

Remote OCP는 SSH SOCKS 터널을 통해 OpenShift 또는 RHOAI 대시보드를 여는 macOS/Windows 데스크톱 앱입니다.

하나의 **Rust + Tauri** 코드베이스에서 macOS 앱과 Windows 실행 파일을 함께 만듭니다.

## 주요 기능

- SSH IP, 사용자, 포트와 대시보드 URL 입력
- 대시보드 URL을 비워두면 원격 서버에서 OpenShift/RHOAI Route 자동 탐색
- 여러 접속 정보를 프로필로 저장하고 다음 실행 때 다시 사용
- 비밀번호와 개인 키 암호는 저장하지 않음
- 내장 Rust SSH 클라이언트와 SOCKS5 프록시 사용
- Google Chrome, Chromium 또는 Microsoft Edge 자동 탐색
- 브라우저가 닫히면 SSH 연결도 자동 종료
- macOS와 Windows에서 같은 화면과 기능 사용

## 사용 방법

1. 운영체제에 맞는 Remote OCP 앱을 실행합니다.
2. SSH IP와 사용자 이름을 입력합니다.
3. 비밀번호 인증이면 SSH 비밀번호를 입력합니다. 개인 키가 있으면 비밀번호를 비워둘 수 있습니다.
4. 대시보드 URL은 필요할 때만 입력합니다. 비워두면 자동으로 탐색합니다.
5. **대시보드 열기**를 누릅니다.
6. 열린 브라우저 창을 닫으면 터널도 종료됩니다.

프로필에는 SSH 비밀번호와 개인 키 암호가 포함되지 않습니다.

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
- `src-tauri/icons/`: macOS/Windows 앱 아이콘
- `.github/workflows/`: macOS/Windows 자동 빌드

실습용 `*.example.com` 주소는 브라우저 인증서 오류를 자동으로 허용합니다. 다른 주소는 고급 설정에서 직접 선택할 수 있습니다.
