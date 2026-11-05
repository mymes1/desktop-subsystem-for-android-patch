#!/usr/bin/env bash
# Usage: ./.github/scripts/release/build-linux-repo.sh --packages <folder> --out <folder>
#                                                      [--existing <folder>]
#
# Builds DSA's Linux package repositories from .deb and .rpm files, signed, laid out to be
# served as static files (GitHub Pages):
#
#   <out>/apt/pool/main/*.deb                     the packages
#   <out>/apt/dists/stable/main/binary-<arch>/    Packages and Packages.gz, per architecture
#   <out>/apt/dists/stable/Release                with InRelease (inline signed) and
#                                                 Release.gpg (detached), which apt checks
#                                                 against the key the .deb installed
#   <out>/rpm/<arch>/*.rpm                         the packages, each signed with rpmsign
#   <out>/rpm/<arch>/repodata/                     createrepo_c metadata, with repomd.xml.asc
#   <out>/dsa-archive-keyring.gpg, <out>/dsa.asc   the public key, binary and armored
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
out=""
existing=""
suite=stable
component=main

die() {
    echo "build-linux-repo: $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --packages) packages="$2"; shift 2 ;;
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

# apt.
apt="$out/apt"
pool="$apt/pool/$component"
mkdir -p "$pool"
if [[ -n "$existing" && -d "$existing/apt/pool/$component" ]]; then
    cp -n "$existing/apt/pool/$component"/*.deb "$pool/" 2>/dev/null || true
fi
debs=("$packages"/*.deb)
for deb in "${debs[@]}"; do
    cp "$deb" "$pool/"
done
pooled=("$pool"/*.deb)
architectures=()
if [[ ${#pooled[@]} -gt 0 ]]; then
    mapfile -t architectures < <(for deb in "${pooled[@]}"; do
        dpkg-deb --field "$deb" Architecture
    done | sort -u)
    rm -rf "$apt/dists"
    for arch in "${architectures[@]}"; do
        dir="$apt/dists/$suite/$component/binary-$arch"
        mkdir -p "$dir"
        (cd "$apt" && dpkg-scanpackages --multiversion --arch "$arch" "pool/$component" /dev/null) \
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
    echo "apt: ${#pooled[@]} packages for ${architectures[*]}"
else
    echo "apt: no .deb packages"
fi

# dnf and zypper.
rpms=("$packages"/*.rpm)
if [[ -n "$existing" && -d "$existing/rpm" ]]; then
    for old in "$existing"/rpm/*/*.rpm; do
        arch_dir="$out/rpm/$(basename "$(dirname "$old")")"
        mkdir -p "$arch_dir"
        cp -n "$old" "$arch_dir/" 2>/dev/null || true
    done
fi
for rpm_file in "${rpms[@]}"; do
    arch="$(rpm -qp --queryformat '%{ARCH}' "$rpm_file")"
    mkdir -p "$out/rpm/$arch"
    cp "$rpm_file" "$out/rpm/$arch/"
    signed="$out/rpm/$arch/$(basename "$rpm_file")"
    # rpmsign runs gpg itself; the wrapper hands it the passphrase the same way.
    rpmsign --addsign --define "_gpg_name $fingerprint" --define "__gpg $gpg_wrapper" "$signed"
done
for arch_dir in "$out"/rpm/*/; do
    arch_dir="${arch_dir%/}"
    createrepo_c --update "$arch_dir"
    sign --armor --detach-sign --output "$arch_dir/repodata/repomd.xml.asc" \
        "$arch_dir/repodata/repomd.xml"
    gpg --batch --verify "$arch_dir/repodata/repomd.xml.asc" "$arch_dir/repodata/repomd.xml"
    echo "rpm: $(basename "$arch_dir")"
done

# GitHub Pages serves the folder as it is.
touch "$out/.nojekyll"
echo "built the repository in $out"
