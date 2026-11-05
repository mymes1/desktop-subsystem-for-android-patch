#!/usr/bin/env bash
# Builds the Linux packages of DSA, a .deb and an .rpm, from programs already built.
#
#   build-packages.sh --bin-dir <cargo release folder> --version <x.y.z> --out <folder>
#                     (--kernel <file> | --placeholder-kernel) [--image-dir <folder>]
#                     [--arch amd64|arm64] [--no-deb] [--no-rpm]
#
# The packages install the same things:
#
#   /usr/lib/dsa/bin/        dsa, dsa-hostd, dsa-devices and the other host programs
#   /usr/bin/dsa             the command line client
#   /usr/lib/dsa/vm/         the guest kernel (bzImage, or Image on arm64)
#   /usr/lib/dsa/vm/image/   the Android image the first start uses, when one is given
#   /etc/dsa/dsa-hostd.json  the service's configuration
#   systemd units            dsa-hostd.service (system) and dsa-devices.service (user)
#   udev rules, modules      /dev/kvm and /dev/vhost-vsock for the service and the user
#   desktop files            DSA Files, and Open, Share and Install in Android for files
#   GeoClue                  DSA's entry, so the desktop's agent can let it read the location
#
# --placeholder-kernel puts an empty kernel in the package, for testing the package before a
# kernel is built. An image folder is one made by dsa/guest/image/make-image.py, with its
# image.json. Without one the package carries no Android image; the service then downloads
# the newest supported image.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin_dir=""
version=""
out=""
kernel=""
placeholder_kernel=0
image_dir=""
arch=""
make_deb=1
make_rpm=1

die() {
    echo "build-packages: $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bin-dir) bin_dir="$2"; shift 2 ;;
        --version) version="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --kernel) kernel="$2"; shift 2 ;;
        --placeholder-kernel) placeholder_kernel=1; shift ;;
        --image-dir) image_dir="$2"; shift 2 ;;
        --arch) arch="$2"; shift 2 ;;
        --no-deb) make_deb=0; shift ;;
        --no-rpm) make_rpm=0; shift ;;
        *) die "unknown argument $1" ;;
    esac
done

[[ -n "$bin_dir" && -d "$bin_dir" ]] || die "name the folder of the built programs with --bin-dir"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "name the version as x.y.z with --version"
[[ -n "$out" ]] || die "name the output folder with --out"
if [[ -z "$arch" ]]; then
    case "$(uname -m)" in
        x86_64) arch=amd64 ;;
        aarch64) arch=arm64 ;;
        *) die "name the architecture with --arch" ;;
    esac
fi
case "$arch" in
    amd64) kernel_name=bzImage; rpm_arch=x86_64 ;;
    arm64) kernel_name=Image; rpm_arch=aarch64 ;;
    *) die "the architecture is amd64 or arm64, not $arch" ;;
esac
if [[ -z "$kernel" && "$placeholder_kernel" -eq 0 ]]; then
    die "name the guest kernel with --kernel (or --placeholder-kernel for a package test)"
fi
[[ -x "$bin_dir/dsa" ]] || die "$bin_dir has no dsa"

package=desktop-subsystem-for-android
mkdir -p "$out"
out="$(cd "$out" && pwd)"
stage="$out/stage"
rm -rf "$stage"
mkdir -p "$stage"

# The programs: every DSA program the build made.
install -d "$stage/usr/lib/dsa/bin" "$stage/usr/bin"
programs=()
for program in "$bin_dir"/dsa "$bin_dir"/dsa-*; do
    [[ -f "$program" && -x "$program" ]] || continue
    case "$program" in *.d | *.so | *.rlib) continue ;; esac
    install -m 0755 "$program" "$stage/usr/lib/dsa/bin/"
    programs+=("$(basename "$program")")
done
ln -s ../lib/dsa/bin/dsa "$stage/usr/bin/dsa"
echo "programs: ${programs[*]}"
has_program() {
    [[ -x "$stage/usr/lib/dsa/bin/$1" ]]
}

# The guest kernel and the Android image.
install -d "$stage/usr/lib/dsa/vm"
if [[ -n "$kernel" ]]; then
    install -m 0644 "$kernel" "$stage/usr/lib/dsa/vm/$kernel_name"
else
    : > "$stage/usr/lib/dsa/vm/$kernel_name"
    chmod 0644 "$stage/usr/lib/dsa/vm/$kernel_name"
fi
system_image=""
if [[ -n "$image_dir" ]]; then
    [[ -f "$image_dir/image.json" ]] || die "$image_dir has no image.json"
    install -d "$stage/usr/lib/dsa/vm/image"
    for file in "$image_dir"/*; do
        install -m 0644 "$file" "$stage/usr/lib/dsa/vm/image/"
    done
    for name in system.img system.vhdx disk.img disk.vhdx; do
        if [[ -f "$stage/usr/lib/dsa/vm/image/$name" ]]; then
            system_image="/usr/lib/dsa/vm/image/$name"
            break
        fi
    done
fi

# The service's configuration.
image_folder=""
if [[ -n "$image_dir" ]]; then
    image_folder=/usr/lib/dsa/vm/image
fi
install -d "$stage/etc/dsa"
cat > "$stage/etc/dsa/dsa-hostd.json" <<EOF
{
  "vm_name": "dsa-android",
  "kernel": "/usr/lib/dsa/vm/$kernel_name",
  "system_image": "$system_image",
  "image_dir": "$image_folder",
  "images_dir": "/var/lib/dsa/images",
  "socket": "/run/dsa/host.sock",
  "data_disk_size_gb": 64,
  "kernel_cmdline": "",
  "processors": 4,
  "control_port": 5000,
  "log_port": 5001,
  "connect_timeout_secs": 120,
  "stop_timeout_secs": 30,
  "doze_delay_secs": 30
}
EOF
chmod 0644 "$stage/etc/dsa/dsa-hostd.json"

# systemd, udev and the kernel modules.
install -d "$stage/usr/lib/systemd/system" "$stage/usr/lib/systemd/user" \
    "$stage/usr/lib/systemd/system-preset" "$stage/usr/lib/systemd/user-preset" \
    "$stage/usr/lib/udev/rules.d" "$stage/usr/lib/modules-load.d"
system_units=()
user_units=()
if has_program dsa-hostd; then
    install -m 0644 "$here/systemd/dsa-hostd.service" "$stage/usr/lib/systemd/system/"
    install -m 0644 "$here/systemd/90-dsa.preset" "$stage/usr/lib/systemd/system-preset/"
    system_units+=(dsa-hostd.service)
fi
if has_program dsa-devices; then
    install -m 0644 "$here/systemd/dsa-devices.service" "$stage/usr/lib/systemd/user/"
    install -m 0644 "$here/systemd/90-dsa-user.preset" "$stage/usr/lib/systemd/user-preset/"
    user_units+=(dsa-devices.service)
fi
install -m 0644 "$here/udev/70-dsa.rules" "$stage/usr/lib/udev/rules.d/"
install -m 0644 "$here/udev/dsa-modules.conf" "$stage/usr/lib/modules-load.d/dsa.conf"

# The desktop: the file actions and DSA's name for GeoClue.
install -d "$stage/usr/share/applications" "$stage/etc/geoclue/conf.d"
install -m 0644 "$here"/desktop/*.desktop "$stage/usr/share/applications/"
install -m 0644 "$here/geoclue/dsa.conf" "$stage/etc/geoclue/conf.d/90-dsa.conf"

# The licence.
install -d "$stage/usr/share/doc/$package"
repo_root="$(cd "$here/../../../../.." && pwd)"
install -m 0644 "$repo_root/LICENSE" "$stage/usr/share/doc/$package/copyright"

# What the package scripts do, the same for both formats.
system_units_text="${system_units[*]:-}"
user_units_text="${user_units[*]:-}"
read -r -d '' after_install <<EOF || true
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload || true
    for unit in $system_units_text; do
        systemctl preset "\$unit" || true
        systemctl restart "\$unit" || true
    done
    for unit in $user_units_text; do
        systemctl --global preset "\$unit" || true
    done
    systemctl restart systemd-modules-load.service || true
fi
for module in vhost_vsock vhost_net tun; do
    modprobe "\$module" 2>/dev/null || true
done
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules || true
    udevadm trigger --subsystem-match=misc --subsystem-match=video4linux || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
EOF
read -r -d '' before_remove <<EOF || true
if [ -d /run/systemd/system ]; then
    for unit in $system_units_text; do
        systemctl stop "\$unit" || true
        systemctl disable "\$unit" || true
    done
    for unit in $user_units_text; do
        systemctl --global disable "\$unit" || true
    done
fi
EOF
read -r -d '' after_remove <<EOF || true
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
EOF

summary="Runs Android apps in windows of their own on the desktop"
description="Desktop Subsystem for Android (DSA) runs Android in a dedicated lightweight VM
and shows each Android task as a real desktop window, with the computer's
sound, cameras, location, sensors, printers and folders lent to its apps."

if [[ "$make_deb" -eq 1 ]]; then
    deb_root="$out/deb"
    rm -rf "$deb_root"
    cp -a "$stage" "$deb_root"
    install -d "$deb_root/DEBIAN"
    size_kb="$(du -sk "$stage" | cut -f1)"
    {
        echo "Package: $package"
        echo "Version: $version"
        echo "Architecture: $arch"
        echo "Maintainer: thisisthepy <noreply@github.com>"
        echo "Installed-Size: $size_kb"
        echo "Depends: libc6, systemd, udev"
        echo "Recommends: pipewire-pulse | pulseaudio, libpulse0, geoclue-2.0, xdg-desktop-portal, cups-client, upower, power-profiles-daemon, iio-sensor-proxy, xdg-utils"
        echo "Section: otherosfs"
        echo "Priority: optional"
        echo "Homepage: https://github.com/thisisthepy/desktop-subsystem-for-android"
        echo "Description: $summary"
        echo "$description" | sed 's/^/ /'
    } > "$deb_root/DEBIAN/control"
    printf '%s\n' /etc/dsa/dsa-hostd.json /etc/geoclue/conf.d/90-dsa.conf > "$deb_root/DEBIAN/conffiles"
    printf '#!/bin/sh\nset -e\n%s\n' "$after_install" > "$deb_root/DEBIAN/postinst"
    printf '#!/bin/sh\nset -e\nif [ "$1" = remove ] || [ "$1" = deconfigure ]; then\n%s\nfi\n' "$before_remove" > "$deb_root/DEBIAN/prerm"
    printf '#!/bin/sh\nset -e\n%s\n' "$after_remove" > "$deb_root/DEBIAN/postrm"
    chmod 0755 "$deb_root/DEBIAN/postinst" "$deb_root/DEBIAN/prerm" "$deb_root/DEBIAN/postrm"
    deb="$out/${package}_${version}_${arch}.deb"
    dpkg-deb --root-owner-group --build "$deb_root" "$deb"
    echo "built $deb"
fi

if [[ "$make_rpm" -eq 1 ]]; then
    top="$out/rpmbuild"
    rm -rf "$top"
    mkdir -p "$top/SPECS" "$top/BUILD" "$top/RPMS" "$top/SOURCES" "$top/SRPMS"
    files_list="$top/files.list"
    (
        cd "$stage"
        find . -mindepth 1 \( -type f -o -type l \) | sed 's|^\.||' | sort | while read -r path; do
            case "$path" in
                /etc/*) echo "%config(noreplace) $path" ;;
                *) echo "$path" ;;
            esac
        done
        echo "%dir /usr/lib/dsa"
        echo "%dir /usr/lib/dsa/bin"
        echo "%dir /usr/lib/dsa/vm"
        [[ -d usr/lib/dsa/vm/image ]] && echo "%dir /usr/lib/dsa/vm/image"
        echo "%dir /etc/dsa"
    ) > "$files_list"
    sed -e "s|@NAME@|$package|g" -e "s|@VERSION@|$version|g" \
        -e "s|@SUMMARY@|$summary|g" -e "s|@STAGE@|$stage|g" -e "s|@FILES@|$files_list|g" \
        "$here/rpm/dsa.spec.in" > "$top/SPECS/dsa.spec"
    {
        echo "%description"
        echo "$description"
        echo
        echo "%post"
        echo "$after_install"
        echo
        echo "%preun"
        echo 'if [ "$1" -eq 0 ]; then'
        echo "$before_remove"
        echo "fi"
        echo
        echo "%postun"
        echo "$after_remove"
    } >> "$top/SPECS/dsa.spec"
    rpmbuild --define "_topdir $top" --target "$rpm_arch" -bb "$top/SPECS/dsa.spec"
    find "$top/RPMS" -name '*.rpm' -exec cp {} "$out/" \;
    echo "built $(ls "$out"/*.rpm)"
fi
