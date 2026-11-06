# Troubleshooting and logs

This page covers Desktop Subsystem for Android™ (DSA) on Windows, and the Linux host where
noted.

## First checks

```
dsa status          # Android's state, any problem, and any pending update
dsa start --wait    # start Android and wait until it has booted
```

`dsa status` says `Active` once Android has booted and DSA's agent in it is connected.
`Stalled` means Android runs but has taken too long; the next line says why and what to
do. `Dozing` is the idle state (see [Memory](memory.md)).

| Problem | What to do |
|---|---|
| Setup says Virtual Machine Platform is off | Turn on the Windows feature and restart |
| No windows appear and `dsa status` names the window manager | Sign out and in again; the window manager starts at sign-in |
| An app opens at a bad size | `dsa app-window <package> reset` |
| Android does not boot after an image update | DSA goes back to the previous image on its own, and `dsa status` says so |
| Android is broken beyond repair | `dsa reset-data` says what would be lost; `dsa reset-data --yes` erases your Android data and starts fresh. The image and settings stay |
| No network in Android | `dsa network` shows the mode, addresses, DNS and any problem |

## Android's log

Android's log stays in memory in Android and is never written to a file. It flows to your
computer only while something reads it.

```
dsa logcat                        # follow it; Ctrl+C ends it
dsa logcat -d -b all *:W          # what it holds now, warnings and above
dsa logcat ActivityManager:I *:S  # one tag only
```

The **Android log** page of DSA Settings shows the same.

## A problem report

```
dsa logs collect -o diag.zip
```

The archive holds Android's whole log, its crash records (tombstones, ANR traces, dropbox,
pstore), a summary of Android's system services, your Android's console, your DSA
processes' logs, and the service's log without other users' lines. `summary.txt` inside
says what could not be gathered. It takes a minute or more. Attach it to an
[issue](https://github.com/thisisthepy/desktop-subsystem-for-android/issues).

## Where logs live on Windows

| Folder | Files |
|---|---|
| `%ProgramData%\DSA\logs\` | `dsa-hostd.log` (the service), `android-<SID>.log` (each user's Android console: early boot and a crash, at most 2 MiB a boot), `setup-<version>.log` (self-updates) |
| `%LOCALAPPDATA%\DSA\logs\` | `dsa-wm.log` (the window manager), `dsa-app-window.log` (app windows) |

The **Advanced** page of DSA Settings opens these folders.

## Where logs live on Linux (not released yet)

The services write to the systemd journal:

```
journalctl -u dsa-hostd
journalctl --user -u dsa-devices -u dsa-compositor
```

## Every command

`dsa` with no arguments lists every command. `--json` prints machine-readable output for
`status`, `start`, `stop`, `settings`, `memory` and `gpus`.

---

Android is a trademark of Google LLC. Windows is a trademark of the Microsoft group of
companies. DSA is not affiliated with, endorsed by or sponsored by Google or Microsoft.
