# 설정 둘러보기

DSA 설정은 Windows용 Desktop Subsystem for Android™(DSA)의 설정 앱입니다. 시작 메뉴에서 엽니다. 테마,
강조색, 언어(영어와 한국어)를 따릅니다. 스스로 바꾸는 것은 없고 모든 스위치가 `dsa` 명령을 거치므로,
`dsa settings`가 같은 값을 보여 주고 `dsa settings set <name> <value>`가 바꿉니다. 스위치는 `on`이나 `off`를
받습니다.

"관리자"라고 적은 설정은 컴퓨터 전체의 것이라 관리자만 바꿉니다.

| 페이지 | 내용 | 명령줄 |
|---|---|---|
| 일반 | Android 시작, 중지, 얼리기, 상태와 마지막 오류 | `dsa start`, `dsa stop`, `dsa freeze`, `dsa status` |
| 메모리 | 메모리 한도(기본 자동), VM 크기, 부분마다 쓰는 양 | `memory-limit`, `dsa memory` |
| 앱 | 설치된 앱: 세부 정보, 앱마다 Android 설정, 제거 | `dsa apps list`, `info`, `settings`, `uninstall` |
| 백그라운드에서 실행 중 | 창 없이 실행되는 앱을 열거나 멈춥니다 | `dsa background`, `stop-without-asking` |
| 위젯 | 바탕 화면 위젯 추가와 제거 | `dsa widgets` |
| 디스플레이 및 GPU | 창을 그리는 GPU | `render-gpu`(`display`, `integrated`, `discrete` 또는 GPU 이름 일부), `dsa gpus` |
| Android | Android 버전과 고를 수 있는 버전 | `dsa android version`, `dsa android use` |
| 업데이트 | DSA와 Android 이미지 업데이트(관리자) | `program-updates`, `image-updates`, `dsa update` |
| 기기 정보 | Android가 앱에 알리는 제조사, 모델, 지문 | `dsa settings device`, `device.<field>`, `dsa settings reset device` |
| 네트워크 | Android의 네트워크 연결 방식, 주소, DNS | `dsa network`, `advanced-networking` |
| 고급 | 유휴 시 얼리기, 웹 링크를 Android 앱에서 열기, 클립보드, 공유 폴더, 개발자 모드와 adb 주소, 추적 스위치(관리자), 로그 폴더 | `freeze`, `links-in-android`, `clipboard`, `file-sharing`, `shared-folders`, `developer-mode`, `adb-port`, `input-trace`, `resize-trace` |
| Android 로그 | Android의 실시간 로그 | `dsa logcat` |
| 정보 | 버전과 라이선스 | |

참고:

- `advanced-networking`(기본값 꺼짐)은 Android를 별도 NAT 네트워크 뒤가 아니라 내 네트워크의 독립 기기로
  둡니다. 다음에 Android를 시작할 때 적용됩니다.
- 기기 정보 변경은 다음에 Android를 시작할 때 적용됩니다.
- `arm-translation off`는 설치된 ARM 변환 패키지를 남겨 두되 Android를 그것 없이 시작합니다.

---

Android는 Google LLC의 상표입니다. Windows는 Microsoft 그룹의 상표입니다. DSA는 Google, Microsoft와
제휴하거나 그들의 보증 또는 후원을 받지 않습니다.
