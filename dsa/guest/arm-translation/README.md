# guest/arm-translation

Most Android apps ship native code for ARM only. The guest is x86_64, so such an app installs
and runs only through a translator that plugs into Android as its native bridge. DSA ships no
translator in the image: the ones Android products use (Houdini, Google's libndk_translation)
are proprietary, and upstream Berberis translates RISC-V only. The user installs one as a
package: the open source default the x86_64 image's release carries (below), one of their
own, or Google's.

A package is a folder holding:

| File | What it is |
|---|---|
| `arm-translation.json` | `name`, `version`, `library` (the bridge in `system/lib64`), `abis` (`["arm64-v8a"]`), `license` |
| `system.img` | an erofs image of `system/lib64`, `system/bin` and `system/etc`, with `system/lib64/libdsa_native_bridge.so` naming the bridge |

```
dsa arm-translation install <folder>   # sends it to Android, used from the next start
dsa arm-translation status             # whether ARM apps run, and the ABIs Android installs for
dsa arm-translation remove
dsa settings set arm-translation off   # keeps the package, boots without it
```

At boot the host adds `androidboot.dsa.arm_translation=on` to the bootconfig only while a
package is installed and the setting is on. The image then imports the native bridge and ABI
properties (`arm-translation-on.prop`) and mounts the package over `/system`
(`dsa-arm-translation.rc`). Without it, the image is unchanged and Android installs x86_64
apps only. The guest has no 32-bit zygote, so only 64-bit ARM apps (`arm64-v8a`) can run.

## The default translator

The "Translator build" workflow (`.github/workflows/translator.yml`) builds Digitalis, an
Apache-2.0 ARM64 backend for Berberis, on GitHub's standard runners, for Android 16
(`android16`) and 17 (`android17`), runs its host tests (`berberis_arm64_host_tests`), and
keeps each version's package as an artifact. Each x86_64 image's release then carries that
package as `arm-translation.zip`, which `image.json` describes under `arm_translation`.
arm64 images carry none: ARM apps run on them as they are. The host does not install the
default package on its own yet; unzipped, it installs like any other package.

A whole Digitalis tree is an AOSP checkout of about 300 GB once built, so the workflow
fetches only the projects `digitalis-lock.json` lists, each at an exact commit and the
large ones in part. The proxies link the framework libraries of DSA's pinned x86_64 image,
and the package takes ICU, BoringSSL and SQLite from the pinned arm64 image of the same
Android version. Everything the native bridge replaces (libc, the linker, the proxied
libraries) is built.

| File | What it is |
|---|---|
| `digitalis-lock.json` | The pins: per version, the Digitalis manifest and AOSP superproject commits, and every project the build fetches |
| `digitalis_lock.py` | Reads and checks the lock |
| `update-lock.py` | Moves the lock to the newest commits of each branch, or pins projects added to it (`--missing`) |
| `assemble-digitalis.py` | Lists the files the build makes, takes libraries from the pinned images, and assembles the bundle |
| `make-package.py` | Turns a translator's bundle into a package (needs `mkfs.erofs`) |
| `build-digitalis.sh` | Builds the same pins in a full Digitalis tree, in Digitalis' own container. It needs a full AOSP build's disk and hours |

```
uv run dsa/guest/arm-translation/update-lock.py --missing
dsa arm-translation install <folder with arm-translation.json and system.img>
```
