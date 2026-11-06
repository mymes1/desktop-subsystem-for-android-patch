# Memory and idle behaviour

Desktop Subsystem for Android™ (DSA) is built so that an idle Android costs your computer
little, and memory Android no longer uses goes back to your computer.

## Doze, the idle state

When every Android window has been closed or minimized for 30 seconds, Android's screen
goes off and Android enters Doze, as a phone does in a pocket. Notifications, alarms and
widgets keep working in Doze's short maintenance windows, and the CPU rests in between.
`dsa status` then says `Dozing`. Showing any Android window wakes it at once.

Android also follows your computer's sleep: it saves its state before the computer
sleeps and sets its clock when it wakes. Its battery follows your computer's battery.

## Freezing (off by default)

Freezing stops Android's processes altogether, which also stops notifications, alarms and
widget updates. That is why it is never done by default.

- `dsa freeze` freezes Android now.
- "Freeze when idle" on the **Advanced** page of DSA Settings, or
  `dsa settings set freeze on`, freezes it after 30 minutes in Doze.
- Showing a window, or input in one, resumes it.

## Memory goes back continuously

- The VM's memory is taken from your computer only when Android first touches it, and the
  pages Android frees are reported and returned to your computer while it runs, not only
  when it stops.
- When no app has been used for a minute, Android moves cold pages to compressed memory
  and drops clean caches, so that memory is free to return. Cached apps are ended when
  Android goes to sleep with no window open.
- A minimized window's program, and the window manager after 30 seconds without a shown
  window, give up the memory they are not using.

There is no fixed memory cap to set: the memory limit is automatic by default. You can set
one on the **Memory** page of DSA Settings or with `dsa settings set memory-limit 4g`
(`auto` puts it back).

## Seeing it

`dsa memory` shows what your Android takes on the host (the VM, the window manager and
each app window) and inside Android (memory in use, cache, app memory, compressed swap,
the largest processes). The **Memory** page of DSA Settings shows the same.

## Goals (not yet met)

The goal with one simple app open is that all of DSA's processes together use at most
400 MiB of private memory, and at most 250 MiB in the end, with each further app adding
at most 80 MiB. These are goals; they are being measured and worked on.


---

Android is a trademark of Google LLC. Windows is a trademark of the Microsoft group of
companies. DSA is not affiliated with, endorsed by or sponsored by Google or Microsoft.
