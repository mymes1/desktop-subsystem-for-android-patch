# 개발자 모드와 adb

Desktop Subsystem for Android™(DSA)는 컴퓨터의 Android Debug Bridge(adb)가 내 Android에 연결하게 해
줍니다. 기본값은 꺼짐입니다.

1. DSA 설정 **고급** 페이지에서 **개발자 모드**를 켜거나 `dsa settings set developer-mode on`을 실행합니다.
2. Android가 실행 중일 때 연결합니다.

   ```
   adb connect 127.0.0.1:58526
   adb devices
   ```

   Android에서 승인할 창은 나오지 않습니다. 포트는 이 컴퓨터에서만 열려 있어 다른 컴퓨터는 연결할 수 없습니다.
   개발자 모드가 켜져 있는 동안 `dsa status`가 주소를 알려 줍니다.
3. 다 쓰면 끕니다: `dsa settings set developer-mode off`.

다른 포트를 쓰려면 `dsa settings set adb-port 5555`를 실행합니다.

adb는 DSA에 들어 있지 않습니다. Android SDK Platform-Tools에서 받으십시오.

DSA는 자기 일에 adb를 쓰지 않습니다. 창, 입력, 설치는 모두 자체 제어 채널로 오가므로, 개발자 모드를 꺼도
모든 기능이 똑같이 동작합니다.

Android 로그를 보는 데에는 adb가 필요 없습니다. [문제 해결](troubleshooting.md)을 보십시오.

---

Android는 Google LLC의 상표입니다. DSA는 Google과 제휴하거나 Google의 보증 또는 후원을 받지 않습니다.
