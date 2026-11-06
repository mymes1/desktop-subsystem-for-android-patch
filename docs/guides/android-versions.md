# Android versions

Desktop Subsystem for Android™ (DSA) supports the newest two Android major versions:
today Android 16 and Android 17.

- A version leaves support when the second newer major ships, so Android 16 is supported
  until Android 18 is released.
- The image updater moves you to a supported version, and never to a lower major version
  on its own.
- Each image is checked against a signed list before it is used. An image that fails to
  boot is rolled back to the previous one.

## Choosing a version

On the **Android** page of DSA Settings, or:

```
dsa android version          # the version that runs, and the ones you can choose
dsa android use 17           # choose a major version
dsa android use latest
```

Moving to an older major version erases your Android data (apps you installed, accounts
and app data), because Android cannot read data a newer version wrote. The command then
needs `--erase-data` to go ahead.

To turn image updates off, an administrator runs `dsa settings set image-updates off`.


---

Android is a trademark of Google LLC. Windows is a trademark of the Microsoft group of
companies. DSA is not affiliated with, endorsed by or sponsored by Google or Microsoft.
