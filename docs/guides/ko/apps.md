# 앱과 창

Desktop Subsystem for Android™(DSA)에서는 Android 앱의 작업 하나하나가 진짜 데스크톱 창입니다. 휴대폰
화면 전체를 보여 주는 에뮬레이터 틀은 없습니다. 이 페이지는 Windows 호스트를 설명합니다. Linux와 macOS
호스트는 진행 중입니다.

## 앱 열기

- **시작 메뉴.** Android가 보여 주는 앱마다 자기 이름과 아이콘으로 시작 메뉴 바로 가기가 생기므로, Android가
  멈춰 있어도 Windows 검색으로 찾을 수 있습니다. 열면 창 관리자와 Android가 꺼져 있을 때 먼저 시작합니다.
- **명령줄.** `dsa apps list`는 설치된 앱을 보여 주고, `dsa launch <package>`는 앱을 엽니다. 예:
  `dsa launch com.android.settings`.
- **Windows 설정.** 앱마다 설정 > 앱 > 설치된 앱에 나타나며, 다른 프로그램처럼 거기서 제거할 수 있습니다.

다른 프로그램이 이미 쓰는 이름이면 바로 가기 이름 뒤에 패키지를 붙입니다. 예: `Clock (com.example.clock)`.
DSA는 자기가 만든 바로 가기만 바꾸거나 지웁니다.

## 창

- 작업마다 자기 창, 작업 표시줄 단추, Alt+Tab 항목을 가지며 앱 자신의 이름과 아이콘을 씁니다. 작업 관리자에도
  앱 이름으로 나타납니다.
- Windows 11 캡션 단추, 스냅 레이아웃, 실시간 크기 조절이 됩니다. 크기를 바꿔도 앱을 다시 만들지 않으며,
  어떤 배율에서도 선명합니다.
- 마우스, 터치, 펜(필압, 지우개, 측면 단추), 키보드가 동작하고, 한국어를 비롯한 Windows 입력기도 씁니다.
  Android 키보드 앱은 없습니다. 입력기의 글자가 앱으로 바로 갑니다.
- 창을 닫으면 Android에 그 작업을 끝내라고 요청합니다.
- `dsa tasks`는 창이 있는 작업을 보여 주고, `dsa stop-app <package>`는 앱을 멈춥니다.
- `dsa app-window <package> --fixed-size on`은 앱 창 크기를 고정하고, `--start-size 1280x800`(또는 `auto`)은
  처음 열리는 크기를 정합니다. `dsa app-window <package> reset`은 둘 다 되돌립니다.
- 내레이터 같은 화면 읽기 프로그램이 UI Automation으로 앱을 읽습니다.

## 알림

Android 알림은 앱 이름과 아이콘을 단 Windows 알림으로 나타납니다. 누르면 앱이 열립니다. 알림을 닫으면 앱이
붙잡아 두는 알림이 아닌 한 Android에서도 닫힙니다. Android가 Doze 중이어도 알림은 계속 옵니다
([메모리](memory.md) 참고).

## 백그라운드에서 실행 중인 앱

창 없이 계속 실행되는 앱(음악 재생기처럼 포그라운드 서비스를 쓰는 앱)은 알림 영역에 아이콘으로 나타납니다.
아이콘은 앱의 아이콘이고, 도구 설명은 그 알림입니다.

- 누르면 앱이 열립니다.
- 오른쪽 단추를 누르면 열기, 알림의 동작, 중지가 나옵니다. 중지는 먼저 묻습니다. "다시 묻지 않음"을 고르면
  `stop-without-asking` 설정이 켜집니다.
- 미디어를 재생하는 앱은 미디어 키와 Windows 미디어 플라이아웃을 씁니다.
- DSA 설정의 "백그라운드에서 실행 중" 페이지와 `dsa background`가 이 앱들을 보여 줍니다.
  `dsa background open <package>`, `dsa background stop <package>`로 하나씩 다룹니다.

## 바탕 화면의 위젯

Android 위젯을 바탕 화면에 하나씩 자기 창으로 올려 둘 수 있습니다. 위젯은 다른 창 아래에 있고, 바탕 화면
보기를 해도 남으며, 작업 표시줄과 Alt+Tab에는 나오지 않습니다.

1. DSA 설정의 **위젯** 페이지에서 위젯을 추가하거나, `dsa widgets list`와 `dsa widgets add <provider>`를
   씁니다.
2. 위쪽 띠를 끌어 옮기고, 가장자리를 끌어 크기를 바꿉니다. DSA가 놓은 자리를 기억합니다.
3. 오른쪽 단추를 누르면 **크기 조정**(위젯이 허용하는 크기)과 **제거**가 나옵니다.

명령줄에서는 `dsa widgets remove <id>`로 제거합니다.

## 링크와 클립보드

- 앱이 여는 웹 링크는 기본 브라우저로 갑니다. 설치된 앱이 자기 것으로 확인한 링크는 그 앱에서 열립니다.
  모든 링크를 Android에 두려면 "웹 링크를 Android 앱에서 열기"(`dsa settings set links-in-android on`)를
  켭니다.
- 복사한 글자는 양쪽으로 공유됩니다. DSA 설정의 스위치나 `dsa settings set clipboard off`로 끕니다.

## ARM 전용 앱

x64에서 ARM 전용 앱을 쓰려면 따로 설치하는 ARM 변환 패키지가 필요합니다:
`dsa arm-translation install <folder>`. 설치 상태는 `dsa arm-translation status`가 알려 줍니다. DSA는 변환
패키지를 함께 제공하지 않습니다.

---

Android는 Google LLC의 상표입니다. Windows는 Microsoft 그룹의 상표입니다. DSA는 Google, Microsoft와
제휴하거나 그들의 보증 또는 후원을 받지 않습니다.
