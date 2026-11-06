# 문제 해결과 로그

이 페이지는 Windows용 Desktop Subsystem for Android™(DSA)를 다루고, Linux 호스트는 따로 표시합니다.

## 먼저 확인할 것

```
dsa status          # Android 상태, 문제, 대기 중인 업데이트
dsa start --wait    # Android를 시작하고 부팅이 끝날 때까지 기다립니다
```

`dsa status`는 Android가 부팅을 마치고 그 안의 DSA 에이전트가 연결되면 `Active`라고 알려 줍니다. `Stalled`는
Android가 실행 중이지만 너무 오래 걸린다는 뜻이며, 다음 줄에 이유와 할 일이 나옵니다. `Dozing`은 유휴
상태입니다([메모리](memory.md) 참고).

| 문제 | 할 일 |
|---|---|
| 설치 프로그램이 가상 머신 플랫폼이 꺼져 있다고 합니다 | Windows 기능을 켜고 다시 시작합니다 |
| 창이 나타나지 않고 `dsa status`가 창 관리자를 언급합니다 | 로그아웃했다가 다시 로그인합니다. 창 관리자는 로그인할 때 시작합니다 |
| 앱이 이상한 크기로 열립니다 | `dsa app-window <package> reset` |
| 이미지 업데이트 뒤 Android가 부팅하지 않습니다 | DSA가 스스로 이전 이미지로 되돌리고 `dsa status`가 알려 줍니다 |
| Android를 고칠 수 없습니다 | `dsa reset-data`가 무엇이 지워지는지 알려 주고, `dsa reset-data --yes`가 Android 데이터를 지우고 새로 시작합니다. 이미지와 설정은 남습니다 |
| Android에 네트워크가 없습니다 | `dsa network`가 방식, 주소, DNS, 문제를 보여 줍니다 |

## Android 로그

Android 로그는 Android 안의 메모리에만 있고 파일로 쓰지 않습니다. 무언가 읽는 동안에만 컴퓨터로 흘러옵니다.

```
dsa logcat                        # 따라가며 봅니다. Ctrl+C로 끝냅니다
dsa logcat -d -b all *:W          # 지금 있는 로그 중 경고 이상
dsa logcat ActivityManager:I *:S  # 태그 하나만
```

DSA 설정의 **Android 로그** 페이지도 같은 내용을 보여 줍니다.

## 문제 보고

```
dsa logs collect -o diag.zip
```

압축 파일에는 Android의 전체 로그, 충돌 기록(tombstone, ANR 추적, dropbox, pstore), Android 시스템 서비스
요약, 내 Android의 콘솔, 내 DSA 프로세스의 로그, 다른 사용자 줄을 뺀 서비스 로그가 들어갑니다. 안의
`summary.txt`가 모으지 못한 것을 알려 줍니다. 1분 이상 걸립니다.
[이슈](https://github.com/thisisthepy/desktop-subsystem-for-android/issues)에 첨부해 주십시오.

## Windows의 로그 위치

| 폴더 | 파일 |
|---|---|
| `%ProgramData%\DSA\logs\` | `dsa-hostd.log`(서비스), `android-<SID>.log`(사용자마다 Android 콘솔: 초기 부팅과 충돌, 부팅마다 최대 2 MiB), `setup-<version>.log`(자체 업데이트) |
| `%LOCALAPPDATA%\DSA\logs\` | `dsa-wm.log`(창 관리자), `dsa-app-window.log`(앱 창) |

DSA 설정의 **고급** 페이지에서 이 폴더를 열 수 있습니다.

## Linux의 로그 위치 (미출시)

서비스는 systemd 저널에 씁니다.

```
journalctl -u dsa-hostd
journalctl --user -u dsa-devices -u dsa-compositor
```

## 모든 명령

인수 없이 `dsa`를 실행하면 모든 명령을 보여 줍니다. `--json`은 `status`, `start`, `stop`, `settings`,
`memory`, `gpus`의 출력을 기계가 읽는 형식으로 냅니다.

---

Android는 Google LLC의 상표입니다. Windows는 Microsoft 그룹의 상표입니다. DSA는 Google, Microsoft와
제휴하거나 그들의 보증 또는 후원을 받지 않습니다.
