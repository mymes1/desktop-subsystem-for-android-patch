#!/usr/bin/env bash
# Usage: ./.github/scripts/release/build-linux-repo.sh --packages <folder> --out <folder>
#                                                      [--prerelease-packages <folder>]
#                                                      [--existing <folder>]
#
# Builds DSA's Linux package repositories from .deb and .rpm files, signed, laid out to be
# served as static files (GitHub Pages):
#
#   <out>/apt/pool/main/stable/*.deb              the stable packages
#   <out>/apt/pool/main/prerelease/*.deb          the pre-releases
#   <out>/apt/dists/<suite>/main/binary-<arch>/   Packages and Packages.gz, per architecture
#   <out>/apt/dists/<suite>/Release               with InRelease (inline signed) and
#                                                 Release.gpg (detached), which apt checks
#                                                 against the key the .deb installed
#   <out>/rpm/<arch>/*.rpm                         the stable packages, each signed with rpmsign
#   <out>/rpm-prerelease/<arch>/*.rpm              every package, stable and pre-release
#   <out>/rpm*/<arch>/repodata/                    createrepo_c metadata, with repomd.xml.asc
#   <out>/dsa-archive-keyring.gpg, <out>/dsa.asc   the public key, binary and armored
#
# There are two release channels. The apt suite stable and rpm/ carry the packages of
# --packages, the stable releases. The suite prerelease and rpm-prerelease/ carry those and
# the pre-releases of --prerelease-packages, so a computer that takes pre-releases is offered
# the higher version of the two. Each package's entry points at one channel, and the DSA
# service points it at the other when the user changes the prerelease-updates setting.
#
# The packages register these repositories when they are installed (build-packages.sh), so
# a new version published here reaches users through apt, dnf, zypper and the software
# center. --existing is a folder that already holds such a repository (the published one):
# its packages are kept beside the new ones, so earlier versions stay installable.
#
# The signing key comes from the environment, never from a file in the repository:
#
#   DSA_REPO_GPG_KEY         the ASCII-armored secret key (a repository secret in CI)
#   DSA_REPO_GPG_PASSPHRASE  its passphrase, when it has one
#
# The key is imported into a GnuPG home made for this run and deleted when it ends.
#
# Needs: gpg, dpkg-scanpackages (dpkg-dev), apt-ftparchive (apt-utils), rpmsign (rpm) and
# createrepo_c.

set -euo pipefail

packages=""
prerelease_packages=""
out=""
existing=""
component=main

die() {
    echo "build-linux-repo: $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --packages) packages="$2"; shift 2 ;;
        --prerelease-packages) prerelease_packages="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --existing) existing="$2"; shift 2 ;;
        *) die "unknown argument $1" ;;
    esac
done

[[ -n "$packages" && -d "$packages" ]] || die "name the folder of .deb and .rpm files with --packages"
[[ -n "$out" ]] || die "name the output folder with --out"
[[ -n "${DSA_REPO_GPG_KEY:-}" ]] || die "DSA_REPO_GPG_KEY holds no signing key"
for tool in gpg dpkg-scanpackages apt-ftparchive rpmsign createrepo_c; do
    command -v "$tool" >/dev/null || die "$tool is needed and is not installed"
done

packages="$(cd "$packages" && pwd)"
if [[ -n "$prerelease_packages" ]]; then
    [[ -d "$prerelease_packages" ]] || die "$prerelease_packages is not a folder"
    prerelease_packages="$(cd "$prerelease_packages" && pwd)"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"

# A GnuPG home of this run only, so the secret key never lands in the runner's own.
GNUPGHOME="$(mktemp -d)"
export GNUPGHOME
chmod 0700 "$GNUPGHOME"
trap 'gpgconf --kill all >/dev/null 2>&1 || true; rm -rf "$GNUPGHOME"' EXIT
passphrase="${DSA_REPO_GPG_PASSPHRASE:-}"
printf 'allow-loopback-pinentry\n' > "$GNUPGHOME/gpg-agent.conf"
printf '%s\n' "$DSA_REPO_GPG_KEY" | gpg --batch --pinentry-mode loopback \
    --passphrase "$passphrase" --import
fingerprint="$(gpg --batch --with-colons --list-secret-keys \
    | awk -F: '$1 == "fpr" { print $10; exit }')"
[[ -n "$fingerprint" ]] || die "DSA_REPO_GPG_KEY holds no secret key"
echo "signing with $fingerprint"
# gpg as the signing steps run it: no prompts, the passphrase from a file of this run.
printf '%s' "$passphrase" > "$GNUPGHOME/passphrase"
chmod 0600 "$GNUPGHOME/passphrase"
gpg_wrapper="$GNUPGHOME/gpg-sign"
{
    echo '#!/bin/sh'
    echo "exec $(command -v gpg) --batch --yes --pinentry-mode loopback --passphrase-file '$GNUPGHOME/passphrase' \"\$@\""
} > "$gpg_wrapper"
chmod 0700 "$gpg_wrapper"
sign() {
    "$gpg_wrapper" --local-user "$fingerprint" "$@"
}

# The public key, for the packages and for anyone adding the repository by hand.
gpg --batch --yes --export "$fingerprint" > "$out/dsa-archive-keyring.gpg"
gpg --batch --yes --armor --export "$fingerprint" > "$out/dsa.asc"

shopt -s nullglob

# apt. Both suites share one pool: pool/main/stable holds the stable packages and
# pool/main/prerelease the pre-releases. The stable suite lists the first folder, and the
# prerelease suite both, so the package manager takes the higher version.
apt="$out/apt"
mkdir -p "$apt/pool/$component/stable" "$apt/pool/$component/prerelease"
if [[ -n "$existing" ]]; then
    for kind in stable prerelease; do
        if [[ -d "$existing/apt/pool/$component/$kind" ]]; then
            cp -n "$existing/apt/pool/$component/$kind"/*.deb "$apt/pool/$component/$kind/" 2>/dev/null || true
        fi
    done
fi
for deb in "$packages"/*.deb; do
    cp "$deb" "$apt/pool/$component/stable/"
done
if [[ -n "$prerelease_packages" ]]; then
    for deb in "$prerelease_packages"/*.deb; do
        cp "$deb" "$apt/pool/$component/prerelease/"
    done
fi

# Writes, signs and checks the apt suite $1 from the packages under pool/main/$2.
apt_suite() {
    local suite="$1" folder="$2"
    local pooled=()
    mapfile -t pooled < <(find "$apt/pool/$component/$folder" -name '*.deb' | sort)
    if [[ ${#pooled[@]} -eq 0 ]]; then
        echo "apt $suite: no .deb packages"
        return
    fi
    local architectures=()
    mapfile -t architectures < <(for deb in "${pooled[@]}"; do
        dpkg-deb --field "$deb" Architecture
    done | sort -u)
    rm -rf "$apt/dists/$suite"
    for arch in "${architectures[@]}"; do
        dir="$apt/dists/$suite/$component/binary-$arch"
        mkdir -p "$dir"
        (cd "$apt" && dpkg-scanpackages --multiversion --arch "$arch" "pool/$component/$folder" /dev/null) \
            > "$dir/Packages"
        gzip -9nk "$dir/Packages"
    done
    (
        cd "$apt/dists/$suite"
        apt-ftparchive \
            -o APT::FTPArchive::Release::Origin="DSA" \
            -o APT::FTPArchive::Release::Label="Desktop Subsystem for Android" \
            -o APT::FTPArchive::Release::Suite="$suite" \
            -o APT::FTPArchive::Release::Codename="$suite" \
            -o APT::FTPArchive::Release::Architectures="${architectures[*]}" \
            -o APT::FTPArchive::Release::Components="$component" \
            release . > Release.new
        mv Release.new Release
        sign --clearsign --output InRelease Release
        sign --armor --detach-sign --output Release.gpg Release
    )
    gpg --batch --verify "$apt/dists/$suite/InRelease"
    echo "apt $suite: ${#pooled[@]} packages for ${architectures[*]}"
}
apt_suite stable stable
# The pool's root: both folders.
apt_suite prerelease ""

# dnf and zypper: rpm/<arch> holds the stable packages, rpm-prerelease/<arch> every package.
# Places rpm $1, signed, in the repository folder $2.
place_rpm() {
    local rpm_file="$1" repo="$2"
    local arch
    arch="$(rpm -qp --queryformat '%{ARCH}' "$rpm_file")"
    mkdir -p "$out/$repo/$arch"
    cp "$rpm_file" "$out/$repo/$arch/"
    # rpmsign runs gpg itself; the wrapper hands it the passphrase the same way.
    rpmsign --addsign --define "_gpg_name $fingerprint" --define "__gpg $gpg_wrapper" \
        "$out/$repo/$arch/$(basename "$rpm_file")"
}
for repo in rpm rpm-prerelease; do
    if [[ -n "$existing" && -d "$existing/$repo" ]]; then
        for old in "$existing/$repo"/*/*.rpm; do
            arch_dir="$out/$repo/$(basename "$(dirname "$old")")"
            mkdir -p "$arch_dir"
            cp -n "$old" "$arch_dir/" 2>/dev/null || true
        done
    fi
done
for rpm_file in "$packages"/*.rpm; do
    place_rpm "$rpm_file" rpm
    place_rpm "$rpm_file" rpm-prerelease
done
if [[ -n "$prerelease_packages" ]]; then
    for rpm_file in "$prerelease_packages"/*.rpm; do
        place_rpm "$rpm_file" rpm-prerelease
    done
fi
for arch_dir in "$out"/rpm/*/ "$out"/rpm-prerelease/*/; do
    arch_dir="${arch_dir%/}"
    createrepo_c --update "$arch_dir"
    sign --armor --detach-sign --output "$arch_dir/repodata/repomd.xml.asc" \
        "$arch_dir/repodata/repomd.xml"
    gpg --batch --verify "$arch_dir/repodata/repomd.xml.asc" "$arch_dir/repodata/repomd.xml"
    echo "$(basename "$(dirname "$arch_dir")"): $(basename "$arch_dir")"
done

# GitHub Pages serves the folder as it is.
touch "$out/.nojekyll"
echo "built the repository in $out"
