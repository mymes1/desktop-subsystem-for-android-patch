# 시작하기

Desktop Subsystem for Android™(DSA)는 자체 커널을 갖춘 전용 경량 가상 머신에서 Android를 실행하고,
Android 앱 하나하나를 데스크톱의 독립된 창으로 보여 줍니다.

## 플랫폼별 상태

| 호스트 | 상태 |
|---|---|
| Windows 11, x64 | 미리보기로 출시했습니다. 기능은 CI에서 테스트하지만 실제 하드웨어에서 손으로 확인하는 작업은 진행 중이므로 거친 부분이 있을 수 있습니다. |
| Windows 11, ARM64 | 진행 중입니다. CI에서 빌드하고 설치하지만 아직 출시하지 않았습니다. |
| Linux, x64와 ARM64 | 진행 중입니다. `.deb`, `.rpm` 패키지를 CI에서 빌드하고 실행하지만 아직 출시하지 않았고, 패키지 저장소도 아직 공개하지 않았습니다. |
| macOS, Apple silicon | 진행 중입니다. `DSA.app`과 `.pkg`, `.dmg`를 CI에서 빌드하지만 출시하지 않았고, 배포용 서명도 아직 없습니다. |

## Windows

x64 Windows 11과 Windows 기능 **가상 머신 플랫폼**이 필요합니다.

1. "Windows 기능 켜기/끄기"에서 **가상 머신 플랫폼**을 켜고 다시 시작합니다. 설치 프로그램은 이 기능을
   확인하고, 꺼져 있으면 안내 메시지와 함께 멈춥니다. 기능을 직접 켜지는 않습니다.
2. [최신 릴리스](https://github.com/thisisthepy/desktop-subsystem-for-android/releases/latest)에서
   `DSA-v<version>-win-x64-setup.exe`를 내려받고, 같은 릴리스의 `SHA256SUMS.txt`와 비교합니다.

   ```
   certutil -hashfile DSA-v<version>-win-x64-setup.exe SHA256
   ```

3. 실행하고 관리자 권한 요청을 승인합니다. 설치 프로그램은 `DsaHost` 서비스, `dsa` 명령, DSA 설정,
   APK 설치 관리자, 첫 Android 이미지를 설치합니다. 설치 파일에 이미지가 없으면 내려받습니다.
4. 로그아웃했다가 다시 로그인하거나, 그냥 앱을 엽니다. DSA 창 관리자(`dsa-wm`)는 로그인할 때 시작하면서
   Android도 함께 시작하므로, 처음 여는 앱이 부팅을 기다리지 않습니다.

DSA는 자체 커널과 자체 VM을 씁니다. WSL 설정, 커널, 배포판은 쓰지도 바꾸지도 않습니다.

Windows 사용자마다 자기 프로필 안에 자기 Android와 데이터 디스크(`%LOCALAPPDATA%\DSA\android.vhdx`)를
따로 갖습니다. 다른 사용자는 읽을 수 없습니다.

동작을 확인합니다.

```
dsa status
dsa launch com.android.settings
```

DSA는 자기 자신과 Android 이미지를 최신으로 유지합니다. 업데이트는 서명되어 있고, 부팅에 실패한 Android
이미지는 마지막으로 정상이던 이미지로 되돌립니다. 무슨 일이 있었는지는 `dsa status`가 알려 줍니다.

## Linux (진행 중, 미출시)

출시되면 Linux용 DSA는 `.deb`(Debian, Ubuntu)와 `.rpm`(Fedora, openSUSE)으로 제공됩니다. KVM
(`/dev/kvm`)과 Wayland 데스크톱이 필요합니다. Android는 crosvm VM에서 실행됩니다.

어느 패키지든 설치하면 Chrome이나 VS Code처럼 DSA 자체의 서명된 패키지 저장소도 등록합니다.

- Debian, Ubuntu: `/etc/apt/sources.list.d/dsa.sources`, 키는 `/usr/share/keyrings/dsa-archive-keyring.gpg`
- Fedora: `/etc/yum.repos.d/dsa.repo`, 키는 `/etc/pki/rpm-gpg/RPM-GPG-KEY-dsa`
- openSUSE: `/etc/zypp/repos.d/dsa.repo`도 등록합니다.

이후 버전은 `apt`, `dnf`, `zypper`나 소프트웨어 센터로 받습니다. Linux에서는 DSA 자체 업데이터가 없습니다.
저장소는 `https://thisisthepy.github.io/desktop-subsystem-for-android/`에서 제공할 예정이며, 아직 공개하지
않았습니다.

첫 설치는 다음과 같습니다.

```
sudo apt install ./desktop-subsystem-for-android_<version>_amd64.deb   # Debian, Ubuntu
sudo dnf install ./desktop-subsystem-for-android-<version>*.rpm       # Fedora
```

패키지는 `dsa-hostd` 시스템 서비스, `dsa-devices`와 `dsa-compositor` 사용자 서비스, `/usr/bin`의 `dsa`
명령, "DSA 파일" 메뉴 항목, 파일용 "열기", "공유", "Android에 설치" 항목을 설치합니다.

## macOS (진행 중, 미출시)

macOS용 DSA는 Apple silicon에서 Apple의 Virtualization 프레임워크로 Android를 실행합니다. `DSA.app`으로
빌드하고, 설치 패키지(`DSA-<version>.pkg`)와 디스크 이미지(`DSA-<version>.dmg`)로 감쌉니다. CI가 둘 다
빌드하고 확인하지만 임시(ad hoc) 서명뿐이라 빌드한 컴퓨터에서만 실행됩니다. Developer ID로 서명한 릴리스는
아직 없습니다.

## 다음

- [앱과 창](apps.md)
- [APK 설치](install-apks.md)

---

Android는 Google LLC의 상표입니다. Windows는 Microsoft 그룹의 상표입니다. macOS는 Apple Inc.의
상표입니다. DSA는 Google, Microsoft, Apple과 제휴하거나 그들의 보증 또는 후원을 받지 않습니다.
