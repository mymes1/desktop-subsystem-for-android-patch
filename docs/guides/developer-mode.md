# Developer mode and adb

Desktop Subsystem for Android™ (DSA) lets the Android Debug Bridge (adb) on your computer
reach your Android. It is off by default.

1. Turn on **Developer mode** on the **Advanced** page of DSA Settings, or run
   `dsa settings set developer-mode on`.
2. With Android running, connect:

   ```
   adb connect 127.0.0.1:58526
   adb devices
   ```

   There is no prompt to accept in Android. The port listens only on this computer, so
   other machines cannot reach it. `dsa status` prints the address while developer mode
   is on.
3. Turn it off when you are done: `dsa settings set developer-mode off`.

To use another port: `dsa settings set adb-port 5555`.

adb itself is not part of DSA. Get it from the Android SDK Platform-Tools.

DSA never uses adb for its own work: windows, input and installs go over its own control
channel, so everything works the same with developer mode off.

For Android's log you do not need adb: see [Troubleshooting](troubleshooting.md).

---

Android is a trademark of Google LLC. DSA is not affiliated with, endorsed by or sponsored
by Google.
