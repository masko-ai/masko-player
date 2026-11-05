#!/usr/bin/env bash
# Builds dav1d, VideoLAN's AV1 decoder, as a static library for the Windows
# player (windows/): target/dav1d/windows-x86_64/lib/dav1d.lib and its
# headers. windows/build.rs links it from MASKO_DAV1D_LIB_DIR, which this
# script prints (and sets for the next steps in GitHub Actions).
#
#   scripts/build-dav1d-windows.sh
#
# It runs in Git Bash on Windows, as GitHub Actions' `shell: bash` on
# windows-latest does. It needs:
#   - Visual Studio's C compiler (MSVC). meson finds it by itself (--vsenv),
#     no Developer Command Prompt needed.
#   - Python 3, to install the pinned meson and ninja into a virtual
#     environment of its own.
#   - nasm 2.14 or later on PATH, for dav1d's x86_64 assembly (in GitHub
#     Actions: `choco install nasm`, then add C:\Program Files\NASM to
#     PATH). Without it dav1d would decode several times slower.
#
# The same dav1d release, checksum, meson and ninja as
# scripts/build-dav1d-apple.sh (read from it), and the same choices:
# release build, static library, 8-bit only (every Masko clip is 8-bit), no
# command-line tools, tests nor examples. The C runtime is the DLL one
# (/MD), as Rust links by default; DAV1D_VSCRT=mt builds for a Rust build
# with +crt-static. x86_64 only for now: arm64 Windows needs dav1d's
# assembly through armasm64 and is not set up yet.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

pinned() { sed -n "s/^$1=\"\\([^\"]*\\)\"\$/\\1/p" scripts/build-dav1d-apple.sh; }
DAV1D_VERSION="$(pinned DAV1D_VERSION)"
DAV1D_SHA256="$(pinned DAV1D_SHA256)"
WANT_MESON="$(pinned WANT_MESON)"
WANT_NINJA="$(pinned WANT_NINJA)"
if [[ -z "$DAV1D_VERSION" || -z "$DAV1D_SHA256" || -z "$WANT_MESON" || -z "$WANT_NINJA" ]]; then
  echo "error: cannot read the pinned versions from scripts/build-dav1d-apple.sh" >&2
  exit 1
fi

ARCH="${1:-x86_64}"
case "$ARCH" in
  x86_64) ;;
  *) echo "error: only x86_64 is set up (asked for $ARCH)" >&2; exit 1 ;;
esac
VSCRT="${DAV1D_VSCRT:-md}"
case "$VSCRT" in
  md | mt) ;;
  *) echo "error: DAV1D_VSCRT is md or mt, not $VSCRT" >&2; exit 1 ;;
esac

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi
}

WORK="$ROOT/target/dav1d"
TARBALL="$WORK/dav1d-$DAV1D_VERSION.tar.xz"
SRC="$WORK/dav1d-$DAV1D_VERSION"
OUT="$WORK/windows-$ARCH"
mkdir -p "$WORK"

# nasm, for the assembly.
if ! command -v nasm >/dev/null && [[ -x "/c/Program Files/NASM/nasm.exe" ]]; then
  export PATH="/c/Program Files/NASM:$PATH"
fi
command -v nasm >/dev/null || {
  echo "error: nasm is not on PATH; install it (choco install nasm) and run again" >&2
  exit 1
}

# meson and ninja, pinned: scripts/setup.sh's .toolchain/meson when it has
# them, else a virtual environment of this build's own.
VENV="$ROOT/.toolchain/meson"
venv_bin() { if [[ -d "$VENV/Scripts" ]]; then echo "$VENV/Scripts"; else echo "$VENV/bin"; fi; }
installed() { "$(venv_bin)/python" -c "import importlib.metadata as m; print(m.version('$1'))" 2>/dev/null || true; }
if [[ "$(installed meson)" != "$WANT_MESON" || "$(installed ninja)" != "$WANT_NINJA" ]]; then
  VENV="$WORK/meson-venv"
fi
if [[ "$(installed meson)" != "$WANT_MESON" || "$(installed ninja)" != "$WANT_NINJA" ]]; then
  echo "==> meson $WANT_MESON and ninja $WANT_NINJA"
  PYTHON="$(command -v python3 || command -v python || true)"
  [[ -n "$PYTHON" ]] || { echo "error: Python 3 is needed to install meson" >&2; exit 1; }
  "$PYTHON" -m venv --clear "$VENV"
  "$(venv_bin)/python" -m pip install --quiet --disable-pip-version-check \
    "meson==$WANT_MESON" "ninja==$WANT_NINJA"
fi
export PATH="$(venv_bin):$PATH"

if [[ ! -f "$TARBALL" ]] || [[ "$(sha256 "$TARBALL")" != "$DAV1D_SHA256" ]]; then
  echo "==> download dav1d $DAV1D_VERSION"
  curl --proto '=https' --tlsv1.2 -sSfL -o "$TARBALL.part" \
    "https://downloads.videolan.org/pub/videolan/dav1d/$DAV1D_VERSION/dav1d-$DAV1D_VERSION.tar.xz"
  mv "$TARBALL.part" "$TARBALL"
fi
have_sha="$(sha256 "$TARBALL")"
if [[ "$have_sha" != "$DAV1D_SHA256" ]]; then
  echo "error: $TARBALL has SHA-256 $have_sha, not $DAV1D_SHA256" >&2
  exit 1
fi
rm -rf "$SRC"
tar -xf "$TARBALL" -C "$WORK"

# THIRD_PARTY_NOTICES.md carries this license text, from scripts/licenses/.
# Line endings aside: Git on Windows may check the copy out with CRLF.
if ! cmp -s <(tr -d '\r' <"$SRC/COPYING") <(tr -d '\r' <scripts/licenses/dav1d-COPYING.txt); then
  echo "error: dav1d $DAV1D_VERSION has another COPYING than scripts/licenses/dav1d-COPYING.txt." >&2
  exit 1
fi

echo "==> dav1d $DAV1D_VERSION for Windows $ARCH (MSVC, /$VSCRT)"
rm -rf "$OUT"
mkdir -p "$OUT"
# The compiler named outright: meson then wraps no ccache around it (the
# runner's Strawberry Perl puts one on PATH, which cannot find cl.exe).
export CC=cl
# Git Bash's own link.exe (a file tool in /usr/bin) must not shadow Visual
# Studio's linker, which sits beside cl.exe.
if cl_path="$(command -v cl 2>/dev/null)"; then
  PATH="$(dirname "$cl_path"):$PATH"
  export PATH
fi
meson setup "$OUT/build" "$SRC" \
  --vsenv \
  --prefix "$OUT/install" --libdir lib \
  --buildtype release \
  -Db_vscrt="$VSCRT" \
  -Ddefault_library=static \
  -Denable_tools=false \
  -Denable_tests=false \
  -Denable_examples=false \
  -Dbitdepths="['8']" \
  -Denable_asm=true >"$OUT/setup.log" || { cat "$OUT/setup.log" >&2; exit 1; }
grep -q "nasm" "$OUT/setup.log" || echo "warning: meson did not report nasm; dav1d may be built without its assembly" >&2
# meson compile, not ninja: with --vsenv it runs ninja inside Visual Studio's
# environment again, where cl.exe is found.
meson compile -C "$OUT/build" --vsenv >"$OUT/build.log" || { cat "$OUT/build.log" >&2; exit 1; }
meson install -C "$OUT/build" --no-rebuild --quiet

# meson names a static library libdav1d.a, also with MSVC; Rust's MSVC
# linker looks for dav1d.lib.
mkdir -p "$OUT/lib"
for built in "$OUT/install/lib/libdav1d.a" "$OUT/install/lib/dav1d.lib"; do
  if [[ -f "$built" ]]; then
    cp "$built" "$OUT/lib/dav1d.lib"
    break
  fi
done
[[ -f "$OUT/lib/dav1d.lib" ]] || { echo "error: the build made no static library" >&2; ls "$OUT/install/lib" >&2; exit 1; }
rm -rf "$OUT/include"
cp -R "$OUT/install/include" "$OUT/include"

if LC_ALL=C grep -a -q -e "$ROOT/" "$OUT/lib/dav1d.lib"; then
  echo "warning: $OUT/lib/dav1d.lib contains a path of this machine" >&2
fi

LIB_DIR="$OUT/lib"
if command -v cygpath >/dev/null; then LIB_DIR="$(cygpath -w "$LIB_DIR")"; fi
if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "MASKO_DAV1D_LIB_DIR=$LIB_DIR" >>"$GITHUB_ENV"
fi
printf '  %s  %s bytes\n' "$OUT/lib/dav1d.lib" "$(wc -c <"$OUT/lib/dav1d.lib" | tr -d ' ')"
echo "Built dav1d $DAV1D_VERSION. Build the player with:"
echo "  MASKO_DAV1D_LIB_DIR='$LIB_DIR' cargo build --release"
