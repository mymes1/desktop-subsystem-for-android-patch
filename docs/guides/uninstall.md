# Uninstalling

## Windows

Uninstall Desktop Subsystem for Android™ (DSA) from Settings > Apps > Installed apps, or
run setup again and choose **Uninstall**.

Uninstalling removes, for every user, signed in or not:

- the program, the `DsaHost` service, the `dsa` command, DSA Settings and the APK Installer;
- the downloaded Android images and setup files;
- the Start menu shortcuts of Android apps, their entries in Installed apps, the
  "Android" entry in Explorer, and the `.apk` association and context menu entries;
- DSA's settings and logs.

It **keeps** each user's Android disk, `%LOCALAPPDATA%\DSA\android.vhdx`, so a later
install finds your apps and data again. To start fresh instead, run `dsa reset-data --yes`
before uninstalling, or delete that file afterwards.

A Start menu moved elsewhere by folder redirection keeps its Android shortcuts, because
the uninstaller cannot read another user's redirection. Delete them by hand.

Your WSL setup is untouched, because DSA never changed it.

## Linux (not released yet)

```
sudo apt remove desktop-subsystem-for-android    # Debian, Ubuntu
sudo dnf remove desktop-subsystem-for-android    # Fedora
```

Removing the package also removes the package repository entry it registered.

---

Android is a trademark of Google LLC. Windows is a trademark of the Microsoft group of
companies. DSA is not affiliated with, endorsed by or sponsored by Google or Microsoft.
