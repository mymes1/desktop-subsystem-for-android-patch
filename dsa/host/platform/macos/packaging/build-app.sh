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
    frameworks="$app/Contents/Frameworks"
    cp "$renderer" "$frameworks/libgfxstream_backend.dylib"
    # The renderer links the Vulkan loader from where it was built (Homebrew). The loader
    # comes along into Frameworks and the renderer is pointed at that copy, so DSA.app runs
    # on a Mac without Homebrew.
    while read -r dependency; do
        case "$dependency" in
            /usr/lib/* | /System/* | @*) continue ;;
        esac
        name="$(basename "$dependency")"
        cp "$dependency" "$frameworks/$name"
        chmod u+w "$frameworks/$name"
        install_name_tool -id "@rpath/$name" "$frameworks/$name"
        install_name_tool -change "$dependency" "@loader_path/$name" \
            "$frameworks/libgfxstream_backend.dylib"
    done < <(otool -L "$renderer" | tail -n +2 | awk '{print $1}')
    # MoltenVK is the Vulkan driver the loader opens; its description sits in Resources,
    # where the window manager points the loader (VK_ICD_FILENAMES).
    moltenvk="${DSA_MOLTENVK:-}"
    if [[ -z "$moltenvk" ]] && command -v brew >/dev/null; then
        moltenvk="$(brew --prefix molten-vk 2>/dev/null)/lib/libMoltenVK.dylib"
    fi
    if [[ -n "$moltenvk" && -f "$moltenvk" ]]; then
        cp "$moltenvk" "$frameworks/libMoltenVK.dylib"
        mkdir -p "$app/Contents/Resources/vulkan/icd.d"
        cat > "$app/Contents/Resources/vulkan/icd.d/MoltenVK_icd.json" <<'JSON'
{
    "file_format_version": "1.0.0",
    "ICD": {
        "library_path": "../../../Frameworks/libMoltenVK.dylib",
        "api_version": "1.2.0",
        "is_portability_driver": true
    }
}
JSON
    else
        echo "MoltenVK was not found (set DSA_MOLTENVK); the renderer needs it installed" >&2
    fi
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
    --scripts "$here/scripts" \
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
