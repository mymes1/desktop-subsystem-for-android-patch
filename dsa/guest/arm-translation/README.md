# guest/arm-translation

Most Android apps ship native code for ARM only. The guest is x86_64, so such an app installs
and runs only through a translator that plugs into Android as its native bridge. DSA ships no
translator yet: the ones Android products use (Houdini, Google's libndk_translation) are
proprietary, and upstream Berberis translates RISC-V only. The user installs one as a package.

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

| File | What it is |
|---|---|
| `build-digitalis.sh` | Fetches Digitalis (an Apache-2.0 ARM64 backend for Berberis) at a pinned revision into `.scratch/digitalis` and builds its binary bundle. It needs a full AOSP build's disk and hours |
| `make-package.py` | Turns a translator's bundle into a package (needs `mkfs.erofs`) |

```
dsa/guest/arm-translation/build-digitalis.sh
uv run dsa/guest/arm-translation/make-package.py \
    --bundle .scratch/digitalis/digitalis/dist/digitalis-prebuilts \
    --name Digitalis --version 17-65e242e --out .scratch/arm-translation-package
dsa arm-translation install .scratch/arm-translation-package
```
