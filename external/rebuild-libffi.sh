#!/usr/bin/env bash
# Rebuild the vendored libffi (external/libffi) into external/artifacts/, which
# the interpreter's build.rs then links. The pruned subtree ships no generated
# ./configure, so this bootstraps with autoreconf first; that heavy toolchain
# lives in external/flake.nix, not the everyday dev shell. Run it rarely:
#
#   nix develop ./external -c ./external/rebuild-libffi.sh
#
# Everyday builds do NOT need this: build.rs falls back to the nix-provided
# libffi ($LIBFFI/$LIBFFI_DEV) when external/artifacts is absent (e.g. on CI).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
src="$here/libffi"
build="$(mktemp -d)"
trap 'rm -rf "$build"' EXIT

cp -r "$src"/. "$build/"
cd "$build"
autoreconf -fi
./configure --disable-shared --enable-static --with-pic
make -j"$(nproc)"

mkdir -p "$here/artifacts/lib" "$here/artifacts/include"
cp .libs/libffi.a "$here/artifacts/lib/"
cp include/ffi.h include/ffitarget.h "$here/artifacts/include/"
echo "rebuilt libffi -> external/artifacts/{lib/libffi.a, include/}"
