#!/bin/sh

# This script installs the Nix package manager on your system by
# downloading a binary distribution and running its installer script
# (which in turn creates and populates /nix).

{ # Prevent execution if this script was only partially downloaded
oops() {
    echo "$0:" "$@" >&2
    exit 1
}

umask 0022

tmpDir="$(mktemp -d -t nix-binary-tarball-unpack.XXXXXXXXXX || \
          oops "Can't create temporary directory for downloading the Nix binary tarball")"
cleanup() {
    rm -rf "$tmpDir"
}
trap cleanup EXIT INT QUIT TERM

require_util() {
    command -v "$1" > /dev/null 2>&1 ||
        oops "you do not have '$1' installed, which I need to $2"
}

case "$(uname -s).$(uname -m)" in
    Linux.x86_64)
        hash=595669eb6db6117135ca8ba6667b273eaf980a47523a92e61ed963556cd547b8
        path=x7svz9b68aj74cslr96hgkf784shxla8/nix-2.34.4-x86_64-linux.tar.xz
        system=x86_64-linux
        ;;
    Linux.i?86)
        hash=ad77c07a7cd7b6a2fa20f2ceaf445031051919f0b8fcf0eed98bf43f07eb1dc3
        path=akqj8w084f2yps9skq5xcnsac35czgc4/nix-2.34.4-i686-linux.tar.xz
        system=i686-linux
        ;;
    Linux.aarch64)
        hash=11f5dca5e2eccd7fca31d84ee16fbe18e8752c45a9ca223a1e55aaa16548a48b
        path=cqawp3wybc5flcnf53zld4rl2b4llgg1/nix-2.34.4-aarch64-linux.tar.xz
        system=aarch64-linux
        ;;
    Linux.armv6l)
        hash=84d5083864e6f9b65e931d1580412412c90ddca341be3bb005355420ee107fbf
        path=n4f3dmpr6nmfbz34phpswvs5a14vchr6/nix-2.34.4-armv6l-linux.tar.xz
        system=armv6l-linux
        ;;
    Linux.armv7l)
        hash=b5d9967f0b98e89f34d76d7f8d12e6456010c4934b2a29d2812a5cd7dcfd4d58
        path=vfpf36qrzmhj11lvi3cn9m831sywbbif/nix-2.34.4-armv7l-linux.tar.xz
        system=armv7l-linux
        ;;
    Linux.riscv64)
        hash=77e93ba1b86c7a786ef54fb0730e9fd927002d2df5a3e2a9aae3cef33d88d688
        path=a50yfhyfkj14mbg0ja7da0jhh2szqqlv/nix-2.34.4-riscv64-linux.tar.xz
        system=riscv64-linux
        ;;
    Darwin.x86_64)
        hash=c79f15a0f9cb1700103bc5ae2b96b8cab58bd53208800a1b477ccb718696b5af
        path=mx3aa4bjvpjvpcchvlvxxzha5gl78va8/nix-2.34.4-x86_64-darwin.tar.xz
        system=x86_64-darwin
        ;;
    Darwin.arm64|Darwin.aarch64)
        hash=f283897495c8e2929deeb822e4b56055f691c8a2aa6ccc966c9a40ed551fa71c
        path=l7gczkvsqmkf3hry4mnxdhn060f67rza/nix-2.34.4-aarch64-darwin.tar.xz
        system=aarch64-darwin
        ;;
    *) oops "sorry, there is no binary distribution of Nix for your platform";;
esac

# Use this command-line option to fetch the tarballs using nar-serve or Cachix
if [ "${1:-}" = "--tarball-url-prefix" ]; then
    if [ -z "${2:-}" ]; then
        oops "missing argument for --tarball-url-prefix"
    fi
    url=${2}/${path}
    shift 2
else
    url=https://releases.nixos.org/nix/nix-2.34.4/nix-2.34.4-$system.tar.xz
fi

tarball=$tmpDir/nix-2.34.4-$system.tar.xz

require_util tar "unpack the binary tarball"
if [ "$(uname -s)" != "Darwin" ]; then
    require_util xz "unpack the binary tarball"
fi

if command -v curl > /dev/null 2>&1; then
    fetch() { curl --fail -L "$1" -o "$2"; }
elif command -v wget > /dev/null 2>&1; then
    fetch() { wget "$1" -O "$2"; }
else
    oops "you don't have wget or curl installed, which I need to download the binary tarball"
fi

echo "downloading Nix 2.34.4 binary tarball for $system from '$url' to '$tmpDir'..."
fetch "$url" "$tarball" || oops "failed to download '$url'"

if command -v sha256sum > /dev/null 2>&1; then
    hash2="$(sha256sum -b "$tarball" | cut -c1-64)"
elif command -v shasum > /dev/null 2>&1; then
    hash2="$(shasum -a 256 -b "$tarball" | cut -c1-64)"
elif command -v openssl > /dev/null 2>&1; then
    hash2="$(openssl dgst -r -sha256 "$tarball" | cut -c1-64)"
else
    oops "cannot verify the SHA-256 hash of '$url'; you need one of 'shasum', 'sha256sum', or 'openssl'"
fi

if [ "$hash" != "$hash2" ]; then
    oops "SHA-256 hash mismatch in '$url'; expected $hash, got $hash2"
fi

unpack=$tmpDir/unpack
mkdir -p "$unpack"
tar -xJf "$tarball" -C "$unpack" || oops "failed to unpack '$url'"

script=$(echo "$unpack"/*/install)

[ -e "$script" ] || oops "installation script is missing from the binary tarball!"
export INVOKED_FROM_INSTALL_IN=1
"$script" "$@"

} # End of wrapping
