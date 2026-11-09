#!/usr/bin/env bash
# Builds the ARM probe APK with the Android SDK's and NDK's own tools, without Gradle: the
# NDK's clang for arm64-v8a, aapt2, zipalign, and apksigner with a throwaway key made for the
# build. The probe is a test app only; nothing ships it.
#
#   dsa/guest/arm-probe/build.sh [out dir]
#
# Needs ANDROID_HOME (or ANDROID_SDK_ROOT) with a platform of API 34 or later and build tools,
# an NDK (ANDROID_NDK_HOME, ANDROID_NDK_LATEST_HOME, or the newest under the SDK's ndk/), zip,
# and a JDK's keytool. The APK is <out dir>/dsa-arm-probe.apk; the default out dir is the
# ignored build/ folder beside this script.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="${1:-$here/build}"
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "$sdk" || ! -d "$sdk" ]]; then
    echo "ANDROID_HOME names no Android SDK" >&2
    exit 1
fi

newest() { ls "$1" | sort -V | tail -1; }
ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}"
if [[ -z "$ndk" || ! -d "$ndk" ]] && [[ -d "$sdk/ndk" ]]; then
    ndk="$sdk/ndk/$(newest "$sdk/ndk")"
fi
clang="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin/clang"
if [[ ! -x "$clang" ]]; then
    echo "no NDK clang at $clang; set ANDROID_NDK_HOME" >&2
    exit 1
fi
tools="$sdk/build-tools/$(newest "$sdk/build-tools")"
platform="$(ls "$sdk/platforms" | grep -E '^android-[0-9]+$' | sort -t- -k2 -n | tail -1)"
api="${platform#android-}"
if (( api < 34 )); then
    echo "the newest installed platform is $platform; the probe needs API 34 or later" >&2
    exit 1
fi
android_jar="$sdk/platforms/$platform/android.jar"

rm -rf "$out"
mkdir -p "$out/apk/lib/arm64-v8a"
out="$(cd "$out" && pwd)"

# arm64-v8a only: an APK with no x86_64 library is installed for ARM and runs translated.
"$clang" --target=aarch64-linux-android34 -shared -fPIC -O2 -Wall -Werror \
    -Wl,-z,max-page-size=16384 -o "$out/apk/lib/arm64-v8a/libdsaarmprobe.so" "$here/probe.c" -llog

"$tools/aapt2" link -o "$out/unaligned.apk" -I "$android_jar" --manifest "$here/AndroidManifest.xml" \
    --min-sdk-version 34 --target-sdk-version 35 --version-code 1 --version-name 1.0
(cd "$out/apk" && zip -q -r "$out/unaligned.apk" lib)
"$tools/zipalign" -f -p 4 "$out/unaligned.apk" "$out/aligned.apk"

keytool -genkeypair -keystore "$out/probe.jks" -storepass dsa-probe -keypass dsa-probe \
    -alias probe -keyalg RSA -keysize 2048 -validity 30 -dname "CN=DSA ARM probe" >/dev/null 2>&1
"$tools/apksigner" sign --ks "$out/probe.jks" --ks-pass pass:dsa-probe --key-pass pass:dsa-probe \
    --out "$out/dsa-arm-probe.apk" "$out/aligned.apk"
echo "$out/dsa-arm-probe.apk"
