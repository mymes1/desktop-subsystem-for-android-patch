# guest/gnss

`dsa-gnss` is the guest's GNSS HAL (`android.hardware.gnss`, version 4). It replaces
Cuttlefish's example GNSS HAL, which reads a made-up position, with the host computer's own
location. It reaches the host through `dsa-channeld`'s binder service, as the power HAL does.

## How a location gets to an app

1. An app with Android's location permission asks for the location. Android's location
   permission stays as on a phone: the framework decides which apps may ask.
2. The framework's GNSS provider starts the GNSS engine. `dsa-gnss` sends the host a
   `LocationRequest` that wants a location, with the interval the framework set.
3. The host runs its location service only from then on: on Windows the window manager, in
   the user's session, starts `Windows.Devices.Geolocation.Geolocator`, so Windows' location
   privacy settings and its "in use" indicator apply to DSA as to any desktop app.
4. Each position comes back as a `LocationFix`, with the accuracy, altitude, speed and
   bearing when the host knows them, and reaches the framework through `gnssLocationCb`.
5. When the framework stops the engine, `dsa-gnss` says no location is wanted any longer, and
   the host stops its location service.

When Windows denies DSA the location (location is off in the privacy settings) or has none to
give, the host sends a `LocationStatus` saying so instead, and no position. `dsa-gnss` logs it
and hands the framework nothing, so apps see a device whose location is not available, as on
a phone with location off. The host tries again every few seconds while a location is wanted,
so turning location on in Windows takes effect without restarting the app.

If `dsa-gnss` dies with a location wanted, `dsa-channeld` tells the host none is wanted, so
the host's location service never runs for a guest that is not listening. A host that connects
again hears the request again.

The host's position has no satellites, measurements, navigation messages, assistance data,
geofencing or batching, so those extensions are not offered; the framework does without them.

## Building and testing

- `build.sh [scratch] [out]` fetches the pinned AIDL definition, binder headers and protobuf
  runtime (`fetch.sh`) and builds the program with the NDK.
- `test.sh` builds and runs the rules on the build machine: the messages against the protocol
  vectors, the values a host position becomes, and when the host is asked for locations.

`make-image.py --gnss-hal <out>` installs it, with its init script, VINTF fragment and label
(`hal_gnss_default`), and removes Cuttlefish's GNSS HAL. `sepolicy/gnss.cil` lets it reach the
daemon.

## Checking on a running guest

- `logcat -s dsa-gnss` shows `serving android.hardware.gnss.IGnss/default`, then
  `subscribed to the control channel`.
- Opening a map app shows `the framework wants the location every 1000 ms; asked the host`,
  and the window manager's log on the host shows `location: Android wants it every 1000 ms;
  running`.
- `dumpsys location` shows the `gps` provider with the host's position.
