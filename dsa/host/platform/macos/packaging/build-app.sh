#!/usr/bin/env bash
# Puts the macOS host's programs together as DSA.app, signs it with the virtualization
# entitlement, and wraps it in an installer package (.pkg) and a disk image (.dmg).
#
# Usage: build-app.sh <version> <programs folder> <output folder> [renderer dylib]
#
# The programs folder holds the release builds (dsa-app-window, dsa, and the service and
# window manager programs when they are built). DSA_SIGN_IDENTITY names the Developer ID
# to sign with; without it the bundle is signed ad hoc, which runs on the machine that
# built it and is ready to be signed for distribution. DSA_INSTALLER_IDENTITY signs the
# package the same way.
set -euo pipefail

version="${1:?the version, such as 0.1.0}"
programs="${2:?the folder of the built programs}"
out_arg="${3:?the output folder}"
renderer="${4:-}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../../../.." && pwd)"
mkdir -p "$out_arg"
out="$(cd "$out_arg" && pwd)"

app="$out/DSA.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks" "$app/Contents/Resources/licenses"

sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

# The window host is required: it is the bundle's executable and the template each app's
# own bundle copies. The others are taken when they were built.
cp "$programs/dsa-app-window" "$app/Contents/MacOS/"
for program in dsa dsa-hostd dsa-wm; do
    if [[ -f "$programs/$program" ]]; then
        cp "$programs/$program" "$app/Contents/MacOS/"
    fi
done
if [[ -n "$renderer" ]]; then
    cp "$renderer" "$app/Contents/Frameworks/libgfxstream_backend.dylib"
fi
cp "$root/LICENSE" "$app/Contents/Resources/licenses/DSA.txt"

identity="${DSA_SIGN_IDENTITY:--}"
sign() {
    codesign --force --timestamp=none --options runtime \
        --entitlements "$here/dsa.entitlements" --sign "$identity" "$@"
}
# Inside out: libraries and helper programs first, the bundle last.
for library in "$app/Contents/Frameworks"/*.dylib; do
    [[ -e "$library" ]] || continue
    codesign --force --timestamp=none --sign "$identity" "$library"
done
for program in "$app/Contents/MacOS"/*; do
    [[ "$(basename "$program")" == dsa-app-window ]] && continue
    sign "$program"
done
sign "$app"
codesign --verify --strict --verbose=2 "$app"

package="$out/DSA-$version.pkg"
installer_identity=()
if [[ -n "${DSA_INSTALLER_IDENTITY:-}" ]]; then
    installer_identity=(--sign "$DSA_INSTALLER_IDENTITY")
fi
pkgbuild --component "$app" --install-location /Applications \
    --identifier org.thisisthepy.dsa --version "$version" \
    ${installer_identity[@]+"${installer_identity[@]}"} "$package"

image="$out/DSA-$version.dmg"
staging="$out/dmg"
rm -rf "$staging" "$image"
mkdir -p "$staging"
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -volname "DSA $version" -srcfolder "$staging" -fs HFS+ -format UDZO "$image"
rm -rf "$staging"

echo "Built $app"
echo "Built $package"
echo "Built $image"
