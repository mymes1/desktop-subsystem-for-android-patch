# 파일

Desktop Subsystem for Android™(DSA)는 컴퓨터와 Android 사이에서 파일을 양쪽으로 옮깁니다. 이 페이지는
Windows 호스트를 설명합니다. Linux에도 같은 기능이 "DSA 파일" 메뉴 항목과 파일용 "열기", "공유",
"Android에 설치" 동작으로 있지만 아직 출시하지 않았습니다.

## 컴퓨터에서 보는 Android의 파일

Android의 공유 저장소는 다음에서 탐색기로 열립니다.

- 탐색기 탐색 창의 **"Android"** 항목. WSL의 "Linux" 항목처럼 "내 PC" 옆에 있습니다. Android가 멈춰 있어도
  되며, 열면 Android를 시작하고 저장소를 기다립니다.
- 시작 메뉴의 **DSA 파일**
- DSA 설정의 **Android 파일** 단추
- `dsa files open` (`dsa files`는 위치를 알려 줍니다)

탐색기는 Windows의 WebDAV 클라이언트(WebClient 서비스)로 연결하며, 처음 열 때 Windows가 이 서비스를
시작합니다. 저장소가 응답하지 않으면 DSA가 다른 폴더를 여는 대신 이유와 함께 오류를 보여 줍니다.

## Android에서 보는 내 폴더

문서, 다운로드, 사진, 음악, 동영상, 바탕 화면 폴더가 Android 공유 저장소의 `Host/<folder>` 아래에 나타나므로,
Android 앱이 거기서 파일을 열고 저장할 수 있습니다. Android는 사용자 자신으로서 이 폴더에 접근하므로
Windows가 파일마다 사용자의 권한을 그대로 적용합니다.

- DSA 설정의 **고급** 페이지나 `dsa settings set shared-folders Documents,Pictures`로 일부만 고릅니다.
- `dsa settings set file-sharing off`로 공유를 끕니다. 그러면 어떤 폴더도 공유하지 않습니다.

이전 버전은 `Host` 대신 `Windows`라는 폴더를 썼습니다. Android는 예전 폴더가 비면 지우므로, 거기 둔 파일은
남습니다.

## Android 앱으로 파일 보내기

- 탐색기에서 파일을 앱 창으로 **끌어다 놓으면** 그 앱으로 갑니다. 창에 놓은 APK는 APK 설치 관리자로 갑니다.
  폴더는 보내지 않습니다.
- 탐색기에서 파일을 오른쪽 단추로 누르고 **Android 앱에서 열기**나 **Android 앱과 공유**를 고릅니다(Windows
  11에서는 "추가 옵션 표시" 아래). Android가 어느 앱으로 받을지 묻습니다.
- `dsa open <file>`, `dsa share <file>`도 같습니다.

이 방법으로는 64 MiB까지 보낼 수 있고, 더 큰 파일은 보내기 전에 거부합니다. 큰 파일은 `Host` 아래의 공유
폴더에 두십시오.

## Android에서 컴퓨터로

Android에서 파일을 `Host/<folder>` 아래에 저장하면 컴퓨터의 그 폴더에 있습니다. 또는 탐색기의 "Android"
항목을 열고 Android 저장소에서 파일을 끌어냅니다.

---

Android는 Google LLC의 상표입니다. Windows는 Microsoft 그룹의 상표입니다. DSA는 Google, Microsoft와
제휴하거나 그들의 보증 또는 후원을 받지 않습니다.
