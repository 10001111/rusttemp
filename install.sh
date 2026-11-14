#!/bin/sh
# RustTemp installer for Linux (x86_64).
#
#     curl -fsSL https://raw.githubusercontent.com/10001111/rusttemp/main/install.sh | sh
#
# Installs the static `cpu`, `gpu` and `rusttemp` programs:
#   as root:        /usr/local/bin
#   otherwise:      ~/.local/bin
#
# Optional settings (environment variables):
#   RUSTTEMP_VERSION=v0.1.0        install a specific release (default: latest)
#   RUSTTEMP_INSTALL_DIR=/opt/bin  install somewhere else
#   RUSTTEMP_TARBALL=/path/rusttemp-x86_64-unknown-linux-musl.tar.gz
#                                  install from a downloaded file; a matching
#                                  .sha256 file next to it is verified when present
#
# Uninstall: delete cpu, gpu and rusttemp from the install folder, e.g.
#     rm ~/.local/bin/cpu ~/.local/bin/gpu ~/.local/bin/rusttemp

set -eu

REPO="10001111/rusttemp"
ASSET="rusttemp-x86_64-unknown-linux-musl.tar.gz"

say() { printf '%s\n' "$*"; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }
die() { printf 'Installation failed: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || die "this installer is for Linux. On Windows use install.ps1 (see the README)."
case "$(uname -m)" in
    x86_64 | amd64) ;;
    *) die "only x86_64 is supported for now (this machine is $(uname -m)). Build from source instead: see the README." ;;
esac

if [ -n "${RUSTTEMP_INSTALL_DIR:-}" ]; then
    dir="$RUSTTEMP_INSTALL_DIR"
elif [ "$(id -u)" = "0" ]; then
    dir="/usr/local/bin"
else
    dir="$HOME/.local/bin"
fi

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        die "neither sha256sum nor shasum is available to verify the download"
    fi
}

download() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$1" -O "$2"
    else
        die "curl or wget is required"
    fi
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT INT TERM
tarball="$work/$ASSET"
expected=""

if [ -n "${RUSTTEMP_TARBALL:-}" ]; then
    say "==> Using local file $RUSTTEMP_TARBALL"
    cp "$RUSTTEMP_TARBALL" "$tarball"
    if [ -f "$RUSTTEMP_TARBALL.sha256" ]; then
        expected="$(cut -d ' ' -f 1 <"$RUSTTEMP_TARBALL.sha256")"
    else
        warn "no .sha256 file next to the tarball; it cannot be verified"
    fi
else
    if [ -n "${RUSTTEMP_VERSION:-}" ]; then
        base="https://github.com/$REPO/releases/download/$RUSTTEMP_VERSION"
    else
        base="https://github.com/$REPO/releases/latest/download"
    fi
    say "==> Downloading $ASSET"
    download "$base/$ASSET" "$tarball" || die "download failed; see https://github.com/$REPO/releases"
    download "$base/$ASSET.sha256" "$tarball.sha256" || die "checksum download failed"
    expected="$(cut -d ' ' -f 1 <"$tarball.sha256")"
fi

if [ -n "$expected" ]; then
    say "==> Verifying SHA-256 checksum"
    actual="$(sha256 "$tarball")"
    [ "$actual" = "$expected" ] || die "checksum mismatch (expected $expected, got $actual); nothing was installed"
    say "    OK"
fi

mkdir -p "$work/files"
tar -xzf "$tarball" -C "$work/files"
payload="$(dirname "$(find "$work/files" -type f -name rusttemp | head -n 1)")"
for p in cpu gpu rusttemp; do
    [ -f "$payload/$p" ] || die "the release archive does not contain $p"
done

say "==> Installing to $dir"
mkdir -p "$dir" || die "cannot create $dir (try: sudo sh, or set RUSTTEMP_INSTALL_DIR)"
for p in cpu gpu rusttemp; do
    install -m 755 "$payload/$p" "$dir/$p" || die "cannot write to $dir (try running with sudo)"
done
say "    Installed $("$dir/rusttemp" --version)"

case ":$PATH:" in
    *":$dir:"*) ;;
    *)
        warn "$dir is not on your PATH. Add this line to ~/.bashrc or ~/.profile, then open a new terminal:"
        say "    export PATH=\"$dir:\$PATH\""
        ;;
esac

say ""
say "Done! Try it now:"
say ""
say "    rusttemp          live CPU monitor, refreshed every 1000 ms (same as rusttemp cpu --watch)"
say "    rusttemp cpu      CPU report: model, temperatures, clocks, power, integrated GPU"
say "    rusttemp gpu      dedicated GPU report"
say "    (shortcuts: cpu, gpu)   add --watch to keep refreshing, --help for options"
say ""
say "CPU power readings need root:  sudo $dir/rusttemp cpu"
