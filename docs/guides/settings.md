# Settings tour

DSA Settings is the settings app of Desktop Subsystem for Android™ (DSA) on Windows. Open
it from the Start menu. It follows your theme, accent colour and language (English and
Korean). It changes nothing on its own: every switch goes through the `dsa` command, so
`dsa settings` shows the same values and `dsa settings set <name> <value>` changes them.
Switches take `on` or `off`.

Settings marked "administrator" belong to the whole computer, so only an administrator
changes them.

| Page | What it holds | Command line |
|---|---|---|
| General | Start, stop and freeze Android, its state and last error | `dsa start`, `dsa stop`, `dsa freeze`, `dsa status` |
| Memory | The memory limit (automatic by default), the VM's size, what each part uses | `memory-limit`, `dsa memory` |
| Apps | Installed apps: details, Android's settings for each, uninstall | `dsa apps list`, `info`, `settings`, `uninstall` |
| Running in the background | Apps that run with no window: open or stop them | `dsa background`, `stop-without-asking` |
| Widgets | Add and remove widgets on the desktop | `dsa widgets` |
| Display & GPU | The GPU windows render on | `render-gpu` (`display`, `integrated`, `discrete` or part of a GPU's name), `dsa gpus` |
| Android | The Android version, and the ones you can choose | `dsa android version`, `dsa android use` |
| Updates | DSA and Android image updates (administrator) | `program-updates`, `image-updates`, `dsa update` |
| Device identity | The manufacturer, model and fingerprint Android reports to apps | `dsa settings device`, `device.<field>`, `dsa settings reset device` |
| Network | How Android reaches the network, its addresses and DNS | `dsa network`, `advanced-networking` |
| Advanced | Freeze when idle, web links in Android apps, clipboard, shared folders, developer mode and its adb address, trace switches (administrator), log folders | `freeze`, `links-in-android`, `clipboard`, `file-sharing`, `shared-folders`, `developer-mode`, `adb-port`, `input-trace`, `resize-trace` |
| Android log | Android's live log | `dsa logcat` |
| About | Version and licences | |

Notes:

- `advanced-networking` (off by default) puts Android on your network as a device of its
  own instead of behind a private NAT network. It applies at the next start of Android.
- Device identity changes apply at the next start of Android.
- `arm-translation off` keeps an installed ARM translation package but starts Android
  without it.

---

Android is a trademark of Google LLC. Windows is a trademark of the Microsoft group of
companies. DSA is not affiliated with, endorsed by or sponsored by Google or Microsoft.
