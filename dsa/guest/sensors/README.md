# guest/sensors

`dsa-sensors` is the guest's sensors HAL (`android.hardware.sensors`, version 2,
`ISensors/default`). It shows the host computer's motion and light sensors to Android:

| Host sensor | Android sensor | Units |
|---|---|---|
| Accelerometer | `TYPE_ACCELEROMETER` | m/s^2 |
| Gyrometer | `TYPE_GYROSCOPE` | rad/s |
| Orientation sensor | `TYPE_ROTATION_VECTOR` | unit quaternion |
| Light sensor | `TYPE_LIGHT` | lux |

A sensor the computer does not have, or that the host may not read, is not in the list, so
apps see the computer as it is.

## How a reading travels

1. When it starts, the HAL connects to the host's sensor port (vsock 5580), which the host's
   window manager serves, and reads the host's sensor list. The framework asks for the list
   once a boot, so the HAL waits up to 20 s for the host before it registers.
2. When an app listens to a sensor, the framework turns it on (`activate`) and sets its rate
   (`batch`). The HAL passes both to the host (`SensorActivate`), which starts the sensor.
   The host runs a sensor only while the framework has it on, and stops every sensor when
   the connection ends.
3. The host converts each reading to Android's units and axes and sends what it has every
   10 ms (`SensorReadings`). The HAL stamps the readings on the guest's boot clock, keeping
   their spacing on the host's, and writes them into the framework's event queue.

The event queue is a fast message queue the framework makes and hands over in `initialize`.
libfmq is not in the NDK, so `src/event_queue.cpp` writes it directly: the read and write
counters, the ring of events and the event flag the framework waits on.

When the host's window manager restarts, the HAL connects again and asks for the sensors the
framework still has on. Direct channels and data injection are not offered: the framework
falls back to the event queue.

## Building and testing

- `build.sh [scratch] [out]` fetches the pinned sources and builds `dsa-sensors` with the NDK.
- `test.sh` builds and runs the rules on the build machine: the sensor port's messages
  against the protocol vectors, the Android sensors the host's become, the requests to the
  host, the events readings become and the writes into the event queue.

`make-image.py --sensors <out>/dsa-sensors` installs it with its init script and VINTF
fragment, in the vendor sensors domain (`hal_sensors_default`), in place of Cuttlefish's
sensors HAL. `sepolicy/sensors.cil` lets it reach the host's sensor port and use the event
queue and the wake lock queue the framework's sensor service hands it, which are memfds of
system_server's; `check-image.py` checks both rules.
