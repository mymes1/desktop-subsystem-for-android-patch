# Files

Desktop Subsystem for Android™ (DSA) moves files between your computer and Android in both
directions. This page describes the Windows host; on Linux the same pieces exist as the
"DSA Files" menu entry and "Open", "Share" and "Install in Android" file actions, which are
not released yet.

## Android's files on your computer

Android's shared storage opens in Explorer from:

- the **"Android"** entry in Explorer's navigation pane, next to "This PC", as WSL's
  "Linux" entry is. It works while Android is stopped: opening it starts Android and
  waits for its storage;
- the **DSA Files** Start menu entry;
- the **Android files** button in DSA Settings;
- `dsa files open` (`dsa files` says where it is).

Explorer reaches it through Windows' WebDAV client (the WebClient service), which Windows
starts the first time. If the storage does not answer, DSA shows an error with the reason
rather than leaving Explorer on another folder.

## Your folders in Android

Your Documents, Downloads, Pictures, Music, Videos and Desktop folders show in Android's
shared storage under `Host/<folder>`, so Android apps can open and save files there.
Android reaches them as you: Windows applies your own rights to every file.

- Choose fewer folders on the **Advanced** page of DSA Settings, or
  `dsa settings set shared-folders Documents,Pictures`.
- Turn sharing off with `dsa settings set file-sharing off`. No folder is shared then.

Earlier versions used a folder named `Windows` instead of `Host`. Android removes the
old folders once they are empty, so files you put there stay.

## Sending a file to an Android app

- **Drag and drop** a file from Explorer onto an app's window. It goes to that app. An APK
  dropped on a window goes to the APK Installer instead. Folders are not sent.
- Right-click a file in Explorer and choose **Open in an Android app** or **Share with an
  Android app** (on Windows 11, under "Show more options"). Android asks which app takes it.
- `dsa open <file>` and `dsa share <file>` do the same.

Files up to 64 MiB can be sent this way; a larger file is refused before anything is sent.
To move larger files, put them in a shared folder under `Host` instead.

## From Android to your computer

Save a file in Android under `Host/<folder>` and it is in that folder on your computer.
Or open the "Android" entry in Explorer and drag files out of Android's storage.


---

Android is a trademark of Google LLC. Windows is a trademark of the Microsoft group of
companies. DSA is not affiliated with, endorsed by or sponsored by Google or Microsoft.
