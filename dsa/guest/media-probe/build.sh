#!/usr/bin/env bash
# Builds the media probe APK with the Android SDK's own tools, without Gradle: javac against
# the newest installed platform's android.jar, d8, aapt2, zipalign, and apksigner with a
# throwaway key made for the build. The probe is a test app only; nothing ships it.
#
#   dsa/guest/media-probe/build.sh [out dir]
#
# Needs ANDROID_HOME (or ANDROID_SDK_ROOT) with a platform of API 34 or later and build
# tools, and a JDK (javac, jar, keytool). The APK is <out dir>/dsa-media-probe.apk; the default
# out dir is the ignored build/ folder beside this script.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="${1:-$here/build}"
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "$sdk" || ! -d "$sdk" ]]; then
    echo "ANDROID_HOME names no Android SDK" >&2
    exit 1
fi

newest() { ls "$1" | sort -V | tail -1; }
tools="$sdk/build-tools/$(newest "$sdk/build-tools")"
platform="$(ls "$sdk/platforms" | grep -E '^android-[0-9]+$' | sort -t- -k2 -n | tail -1)"
api="${platform#android-}"
if (( api < 34 )); then
    echo "the newest installed platform is $platform; the probe needs API 34 or later" >&2
    exit 1
fi
android_jar="$sdk/platforms/$platform/android.jar"

rm -rf "$out"
mkdir -p "$out/classes" "$out/dex"
out="$(cd "$out" && pwd)"

javac -source 11 -target 11 -Xlint:-options -encoding UTF-8 -classpath "$android_jar" -d "$out/classes" \
    $(find "$here/src" -name '*.java')
"$tools/d8" --release --min-api 34 --lib "$android_jar" --output "$out/dex" $(find "$out/classes" -name '*.class')

"$tools/aapt2" link -o "$out/unaligned.apk" -I "$android_jar" --manifest "$here/AndroidManifest.xml" \
    --min-sdk-version 34 --target-sdk-version 35 --version-code 1 --version-name 1.0
(cd "$out/dex" && jar uf "$out/unaligned.apk" classes.dex)
"$tools/zipalign" -f -p 4 "$out/unaligned.apk" "$out/aligned.apk"

keytool -genkeypair -keystore "$out/probe.jks" -storepass dsa-probe -keypass dsa-probe \
    -alias probe -keyalg RSA -keysize 2048 -validity 30 -dname "CN=DSA media probe" >/dev/null 2>&1
"$tools/apksigner" sign --ks "$out/probe.jks" --ks-pass pass:dsa-probe --key-pass pass:dsa-probe \
    --out "$out/dsa-media-probe.apk" "$out/aligned.apk"
echo "$out/dsa-media-probe.apk"
