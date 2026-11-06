# Apps and windows

In Desktop Subsystem for Android™ (DSA), every Android app task is a real desktop window.
There is no emulator frame showing a whole phone screen. This page describes the Windows
host; the Linux and macOS hosts are in progress.

## Opening an app

- **Start menu.** Each app Android lists gets a Start menu shortcut under its own name and
  icon, so Windows search finds it, even while Android is stopped. Opening it starts the
  window manager and Android first when they are not running.
- **Command line.** `dsa apps list` shows the installed apps, and
  `dsa launch <package>` opens one, for example `dsa launch com.android.settings`.
- **Windows Settings.** Each app is listed under Settings > Apps > Installed apps, where it
  can be uninstalled like any other program.

A shortcut whose name another program already uses gets the package after it, such as
`Clock (com.example.clock)`. DSA only changes or removes shortcuts it made itself.

## Windows

- Each task has its own window, taskbar button and Alt+Tab entry, with the app's own name
  and icon. Task Manager lists each app's window program under the app's name.
- Windows 11 caption buttons, snap layouts and live resize work. Resizing does not
  recreate the app, and output stays sharp at every display scale.
- Mouse, touch, pen (pressure, eraser, barrel button) and the keyboard work, including the
  Windows input method for Korean and other languages. There is no Android keyboard app:
  text goes from your input method to the app directly.
- Closing a window asks Android to end its task.
- `dsa tasks` lists the tasks that have a window. `dsa stop-app <package>` stops an app.
- `dsa app-window <package> --fixed-size on` keeps an app's windows at one size, and
  `--start-size 1280x800` (or `auto`) sets the size they open at.
  `dsa app-window <package> reset` puts both back.
- Screen readers such as Narrator read the app through UI Automation.

## Notifications

Android notifications show as Windows notifications under the app's name and icon.
Clicking one opens the app. Dismissing one dismisses it in Android too, unless the app
keeps it. Notifications keep arriving while Android is in Doze (see [Memory](memory.md)).

## Apps running in the background

An app that keeps running with no window (because it runs a foreground service, such as a
music player) shows as an icon in the notification area, with the app's icon and its
notification as the tooltip.

- Click it to open the app.
- Right-click it for Open, the notification's actions, and Stop. Stop asks first; "Do not
  ask again" turns on the `stop-without-asking` setting.
- Apps that play media get the media keys and Windows' media flyout.
- The "Running in the background" page of DSA Settings, and `dsa background`, list them;
  `dsa background open <package>` and `dsa background stop <package>` act on one.

## Widgets on the desktop

Android widgets can sit on your desktop, each in a window of its own. A widget stays
below other windows, stays when you show the desktop, and is not in the taskbar or
Alt+Tab.

1. Open the **Widgets** page of DSA Settings and add a widget, or use
   `dsa widgets list` and `dsa widgets add <provider>`.
2. Drag the band along its top to move it, and its edges to resize it. DSA remembers where
   you put it.
3. Right-click it for **Resize** (the sizes the widget allows) and **Remove**.

`dsa widgets remove <id>` removes one from the command line.

## Links and clipboard

- Web links that apps open go to your default browser. Links an installed app has
  verified as its own open in that app. To keep every link in Android, turn on "Open web
  links in Android apps" (`dsa settings set links-in-android on`).
- Copied text is shared both ways. Turn it off with the switch in DSA Settings or
  `dsa settings set clipboard off`.

## ARM-only apps

On x64, apps built only for ARM need an ARM translation package, which is installed
separately: `dsa arm-translation install <folder>`. `dsa arm-translation status` says
what is installed. DSA does not ship one.


---

Android is a trademark of Google LLC. Windows is a trademark of the Microsoft group of
companies. DSA is not affiliated with, endorsed by or sponsored by Google or Microsoft.
