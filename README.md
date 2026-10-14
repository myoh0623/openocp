# Remote OCP for macOS

기존 `open-dashboard.sh`를 macOS 네이티브 GUI 앱으로 옮긴 버전입니다.

사용자가 제공한 OCP 로고를 macOS 앱 아이콘으로 사용합니다.

## 실행

`dist/Remote OCP.app`을 더블클릭합니다.

1. SSH IP와 사용자 이름을 입력합니다.
2. 대시보드 URL을 비워두면 원격 서버에서 OpenShift 콘솔 주소를 자동으로 탐색합니다.
3. **접속 프로필**에 이름을 입력하고 **저장**을 누르면 모든 접속 설정이 보관됩니다. 다음 실행 때 마지막 프로필이 자동으로 선택됩니다.
4. 필요하면 고급 설정에서 URL, Chrome 경로, 프로필 경로 등을 직접 지정합니다.
5. **대시보드 열기**를 누릅니다.
6. SSH 키가 없으면 나타나는 macOS 창에 SSH 비밀번호를 입력합니다.
7. 열린 Chrome 창을 닫으면 SSH 터널도 자동으로 종료됩니다.

SSH 비밀번호는 프로필에 저장하지 않습니다.

실습용 `*.example.com` 주소는 Chrome 인증서 오류를 자동으로 무시합니다. Chrome 내부 진단 메시지는 앱의 실행 기록에 표시하지 않습니다.

## 다시 빌드

macOS 13 이상과 Xcode Command Line Tools가 필요합니다.

```bash
./build-app.sh
```

Apple Silicon과 Intel Mac에서 모두 실행 가능한 Universal 앱과 `Remote-OCP-macOS.zip` 파일이 `dist`에 생성됩니다.

처음 다른 Mac에서 실행할 때 macOS가 개발자를 확인할 수 없다고 표시하면 앱을 Control-클릭한 뒤 **열기**를 선택합니다.
