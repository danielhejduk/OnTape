#!/usr/bin/env bash
# Build a static libasound (alsa-lib) with musl-gcc and install it into $1.
# The resulting static binary still reads the host's ALSA configuration from
# /usr/share/alsa at runtime.
set -euo pipefail

prefix="${1:?usage: $0 <install-prefix>}"
version="${ALSA_VERSION:-1.2.14}"
src="$(mktemp -d)"

curl -fsSL "https://www.alsa-project.org/files/pub/lib/alsa-lib-${version}.tar.bz2" \
  | tar -xj -C "$src" --strip-components=1

# musl-gcc doesn't search the glibc include path, so expose just the Linux
# kernel UAPI headers (linux/, asm/, asm-generic/) that alsa-lib needs.
kinc="$src/kernel-headers"
mkdir -p "$kinc"
triplet="$(gcc -print-multiarch)"
ln -s /usr/include/linux "$kinc/linux"
ln -s /usr/include/asm-generic "$kinc/asm-generic"
ln -s "/usr/include/$triplet/asm" "$kinc/asm"

cd "$src"
CC=musl-gcc CFLAGS="-O2 -I$kinc" ./configure \
  --prefix="$prefix" \
  --enable-static \
  --disable-shared \
  --disable-python \
  --disable-ucm \
  --disable-topology \
  --without-debug \
  --with-configdir=/usr/share/alsa \
  --with-plugindir=/usr/lib/alsa-lib

make -j"$(nproc)"
make install-exec
make -C include install
make -C utils install-pkgconfigDATA 2>/dev/null || install -Dm644 utils/alsa.pc "$prefix/lib/pkgconfig/alsa.pc"

test -f "$prefix/lib/libasound.a"
test -f "$prefix/lib/pkgconfig/alsa.pc"
