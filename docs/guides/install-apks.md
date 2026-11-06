# Installing APKs

Desktop Subsystem for Android™ (DSA) does not ship an app store. You install apps from APK
files. This page describes the Windows host.

## With the APK Installer

Double-click an `.apk` file in Explorer. It opens in the **APK Installer**, a program of
its own with its own taskbar group.

The window shows what the APK is before Android even starts: the app's name, package,
publisher (the signing certificate, since an APK names no publisher), version and icon, the
permissions it asks for under **Capabilities**, and more under **Details** (version code,
the Android versions it is made for, size, processor types, the SHA-256 of its
certificate). An APK with no native code for this PC's processor carries a warning.

When Android is running, the window also says whether this is a new install, an update, or
something Android would refuse (a downgrade, or an app signed with another key), with the
reason.

Click **Install** (or **Update**, **Reinstall**). Android starts first when it is not
running. With **Launch when ready** ticked (the default), the app opens when it is
installed and the installer closes.

You can also drop APK files on the APK Installer's window or on DSA Settings. Each opens in
its own installer window. An APK dropped on an app window goes to the installer too.

## From the command line

```
dsa apk-info app.apk          # what the APK is, read on this PC without Android
dsa install app.apk           # install it
dsa install app.apk --progress
```

`dsa install` exits with code 1 when Android refuses the APK, and its line starts with
`not installed (<reason>):`.

## Why an install is refused

| Reason | Meaning |
|---|---|
| `incompatible` | The APK cannot run on this Android or this processor |
| `conflicting-signature` | An app with this package is installed, signed with another key |
| `downgrade` | The installed version is newer |
| `invalid-apk` | The file is not a valid APK |
| `insufficient-storage` | Android has no room for it |
| `blocked` | Android's policy blocks it |
| `other` | Anything else; Android's own message follows |

APKs up to 2 GiB are accepted, one install at a time. An installed app appears in the
Start menu like every other app.

## Removing an app

Uninstall it from Windows Settings > Apps > Installed apps, from the **Apps** page of DSA
Settings, or with `dsa apps uninstall <package>`.


---

Android is a trademark of Google LLC. Windows is a trademark of the Microsoft group of
companies. DSA is not affiliated with, endorsed by or sponsored by Google or Microsoft.
