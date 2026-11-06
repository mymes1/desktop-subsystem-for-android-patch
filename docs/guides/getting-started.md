# Getting started

Desktop Subsystem for Android™ (DSA) runs Android in a lightweight virtual machine of its
own, with its own kernel, and shows each Android app in a window of its own on your
desktop.

## Status by platform

| Host | Status |
|---|---|
| Windows 11, x64 | Preview, released. Expect rough edges: features are tested in CI, and checking them by hand on real hardware is still in progress. |
| Windows 11, ARM64 | In progress. Builds and installs in CI; not released yet. |
| Linux, x64 and ARM64 | In progress. The `.deb` and `.rpm` packages build and run in CI; not released yet, and the package repository is not published yet. |
| macOS, Apple silicon | In progress. `DSA.app`, its `.pkg` and its `.dmg` build in CI; not released, and not signed for distribution yet. |

## Windows

You need Windows 11 on x64 and the Windows feature **Virtual Machine Platform**.

1. Open "Turn Windows features on or off", tick **Virtual Machine Platform**, and restart.
   Setup checks for it and stops with a message when it is off. It never turns the feature
   on by itself.
2. Download `DSA-v<version>-win-x64-setup.exe` from the
   [latest release](https://github.com/thisisthepy/desktop-subsystem-for-android/releases/latest).
   Check it against `SHA256SUMS.txt` from the same release:

   ```
   certutil -hashfile DSA-v<version>-win-x64-setup.exe SHA256
   ```

3. Run it and approve the administrator prompt. Setup installs the `DsaHost` service, the
   `dsa` command, DSA Settings, the APK Installer, and the first Android image
   (downloaded when setup does not carry one).
4. Sign out and in again, or just open an app. The DSA Window Manager (`dsa-wm`) starts at
   sign-in and starts Android with it, so the first app you open does not wait for a boot.

DSA ships its own kernel and its own VM. It does not use or change your WSL settings,
kernel or distributions.

Each Windows user gets their own Android and their own data disk, in their own profile
(`%LOCALAPPDATA%\DSA\android.vhdx`). Other users cannot read it.

Check that it works:

```
dsa status
dsa launch com.android.settings
```

DSA keeps itself and the Android image up to date. Updates are signed; an Android image
that fails to boot is rolled back to the last good one. `dsa status` says what happened.

## Linux (in progress, not released)

When it is released, DSA for Linux comes as a `.deb` (Debian, Ubuntu) and an `.rpm`
(Fedora, openSUSE). It needs KVM (`/dev/kvm`) and a Wayland desktop. Android runs in a
crosvm VM.

Installing either package also registers DSA's own signed package repository, the way
Chrome and VS Code do:

- Debian and Ubuntu: `/etc/apt/sources.list.d/dsa.sources`, with the key in
  `/usr/share/keyrings/dsa-archive-keyring.gpg`;
- Fedora: `/etc/yum.repos.d/dsa.repo`, with the key in `/etc/pki/rpm-gpg/RPM-GPG-KEY-dsa`;
- openSUSE: also `/etc/zypp/repos.d/dsa.repo`.

Later versions then arrive through `apt`, `dnf`, `zypper` or the software center. DSA
has no updater of its own on Linux. The repository is to be served from
`https://thisisthepy.github.io/desktop-subsystem-for-android/`; it is not published yet.

The first install will look like this:

```
sudo apt install ./desktop-subsystem-for-android_<version>_amd64.deb   # Debian, Ubuntu
sudo dnf install ./desktop-subsystem-for-android-<version>*.rpm       # Fedora
```

The package installs the `dsa-hostd` system service and the `dsa-devices` and
`dsa-compositor` user services, the `dsa` command in `/usr/bin`, the "DSA Files" menu
entry, and "Open", "Share" and "Install in Android" entries for files.

## macOS (in progress, not released)

DSA for macOS runs Android on Apple's Virtualization framework, on Apple silicon. It is
built as `DSA.app`, wrapped in an installer package (`DSA-<version>.pkg`) and a disk
image (`DSA-<version>.dmg`). CI builds and checks both, but they are signed only ad hoc,
so they run only on the machine that built them. A release signed with a Developer ID is
not available yet.

## Next

- [Apps and windows](apps.md)
- [Installing APKs](install-apks.md)


---

Android is a trademark of Google LLC. Windows is a trademark of the Microsoft group of
companies. macOS is a trademark of Apple Inc. DSA is not affiliated with, endorsed by or
sponsored by Google, Microsoft or Apple.
