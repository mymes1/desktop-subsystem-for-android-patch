# APK 설치

Desktop Subsystem for Android™(DSA)는 앱 스토어를 함께 제공하지 않습니다. 앱은 APK 파일로 설치합니다.
이 페이지는 Windows 호스트를 설명합니다.

## APK 설치 관리자로

탐색기에서 `.apk` 파일을 두 번 누릅니다. 작업 표시줄에서 따로 묶이는 독립 프로그램인 **APK 설치 관리자**가
열립니다.

창은 Android를 시작하기 전부터 APK가 무엇인지 보여 줍니다. 앱 이름, 패키지, 게시자(APK에는 게시자 이름이
없으므로 서명 인증서), 버전, 아이콘이 위에 있고, **기능** 아래에 요청하는 권한이, **세부 정보** 아래에 버전 코드,
대상 Android 버전, 크기, 프로세서 종류, 인증서의 SHA-256이 있습니다. 이 PC의 프로세서용 네이티브 코드가 없는
APK에는 경고가 붙습니다.

Android가 실행 중이면 새로 설치인지, 업데이트인지, Android가 거부할 경우(버전 낮추기나 다른 키로 서명한
앱)인지도 이유와 함께 알려 줍니다.

**설치**(또는 **업데이트**, **다시 설치**)를 누릅니다. Android가 꺼져 있으면 먼저 시작합니다. **준비되면
실행**이 켜져 있으면(기본값) 설치가 끝난 뒤 앱이 열리고 설치 관리자가 닫힙니다.

APK 파일을 APK 설치 관리자 창이나 DSA 설정 창에 끌어다 놓아도 됩니다. APK마다 자기 설치 관리자 창이 열립니다.
앱 창에 놓은 APK도 설치 관리자로 갑니다.

## 명령줄에서

```
dsa apk-info app.apk          # Android 없이 이 PC에서 APK를 읽어 보여 줍니다
dsa install app.apk           # 설치합니다
dsa install app.apk --progress
```

Android가 APK를 거부하면 `dsa install`은 종료 코드 1로 끝나고, 출력 줄은 `not installed (<reason>):`로
시작합니다.

## 설치가 거부되는 이유

| 이유 | 뜻 |
|---|---|
| `incompatible` | 이 Android나 이 프로세서에서 실행할 수 없습니다 |
| `conflicting-signature` | 같은 패키지의 앱이 다른 키로 서명되어 설치되어 있습니다 |
| `downgrade` | 설치된 버전이 더 새롭습니다 |
| `invalid-apk` | 올바른 APK가 아닙니다 |
| `insufficient-storage` | Android에 공간이 부족합니다 |
| `blocked` | Android의 정책이 막았습니다 |
| `other` | 그 밖의 경우입니다. Android의 메시지가 뒤에 붙습니다 |

APK는 2 GiB까지 받으며, 한 번에 하나씩 설치합니다. 설치한 앱은 다른 앱처럼 시작 메뉴에 나타납니다.

## 앱 제거

Windows 설정 > 앱 > 설치된 앱, DSA 설정의 **앱** 페이지, 또는 `dsa apps uninstall <package>`로 제거합니다.

---

Android는 Google LLC의 상표입니다. Windows는 Microsoft 그룹의 상표입니다. DSA는 Google, Microsoft와
제휴하거나 그들의 보증 또는 후원을 받지 않습니다.
