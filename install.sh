#!/bin/sh
# Installs unsilo from the GitHub releases.
#
# Downloading with curl rather than a browser matters on macOS: a browser marks
# what it saves with com.apple.quarantine and Gatekeeper then kills the binary,
# while curl does not set that attribute at all. That is why this exists instead
# of a signing certificate.
#
#   curl -fsSL https://raw.githubusercontent.com/Fluzko/unsilo/main/install.sh | sh
#
#   UNSILO_VERSION=v0.1.0     install a specific release rather than the latest
#   UNSILO_INSTALL_DIR=~/bin  install somewhere other than ~/.local/bin

set -eu

REPO="Fluzko/unsilo"
INSTALL_DIR="${UNSILO_INSTALL_DIR:-$HOME/.local/bin}"

die() {
	printf 'install.sh: %s\n' "$1" >&2
	exit 1
}

need() {
	command -v "$1" >/dev/null 2>&1 || die "$1 is required and was not found"
}

need curl
need tar
need uname

# The target triples here are exactly the ones the release workflow builds. A
# platform that is not one of them gets told so, rather than downloading
# something that cannot run.
detect_target() {
	os="$(uname -s)"
	arch="$(uname -m)"
	case "$os/$arch" in
	# macOS reports arm64, Linux reports aarch64, for the same architecture.
	Darwin/arm64 | Darwin/aarch64) echo "aarch64-apple-darwin" ;;
	Darwin/x86_64) echo "x86_64-apple-darwin" ;;
	# The x86_64 Linux build is static, so it does not care which libc is here.
	Linux/x86_64 | Linux/amd64) echo "x86_64-unknown-linux-musl" ;;
	Linux/aarch64 | Linux/arm64) echo "aarch64-unknown-linux-gnu" ;;
	MINGW* | MSYS* | CYGWIN*)
		die "Windows is not covered by this script. Take the .tar.gz for x86_64-pc-windows-msvc from https://github.com/$REPO/releases/latest"
		;;
	*) die "no release is built for $os on $arch. See https://github.com/$REPO/releases/latest" ;;
	esac
}

latest_version() {
	# Read the tag out of the API response without requiring jq.
	curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
		sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' |
		head -n 1
}

checksum() {
	# macOS ships shasum, most Linux images ship sha256sum, and some have both.
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | cut -d' ' -f1
	else
		die "neither sha256sum nor shasum was found, so the download cannot be verified"
	fi
}

TARGET="$(detect_target)"
VERSION="${UNSILO_VERSION:-$(latest_version)}"
[ -n "$VERSION" ] || die "could not work out the latest version. Set UNSILO_VERSION to pick one."

ARCHIVE="unsilo-$TARGET.tar.gz"
BASE="https://github.com/$REPO/releases/download/$VERSION"

TMP="$(mktemp -d)"
# Runs on any exit, so a failed download leaves nothing behind.
trap 'rm -rf "$TMP"' EXIT INT TERM

printf 'unsilo %s for %s\n' "$VERSION" "$TARGET"

curl -fsSL -o "$TMP/$ARCHIVE" "$BASE/$ARCHIVE" ||
	die "could not download $BASE/$ARCHIVE"
curl -fsSL -o "$TMP/$ARCHIVE.sha256" "$BASE/$ARCHIVE.sha256" ||
	die "could not download the checksum for $ARCHIVE"

# Verified before anything is unpacked, so a corrupted or tampered archive is
# never written to disk as an executable.
expected="$(cut -d' ' -f1 <"$TMP/$ARCHIVE.sha256")"
actual="$(checksum "$TMP/$ARCHIVE")"
[ "$expected" = "$actual" ] || die "checksum mismatch: expected $expected, got $actual"
printf '  checksum ok\n'

tar -xzf "$TMP/$ARCHIVE" -C "$TMP"
BINARY="$TMP/unsilo-$TARGET/unsilo"
[ -f "$BINARY" ] || die "the archive did not contain the binary where expected"

previous=""
if command -v unsilo >/dev/null 2>&1; then
	previous="$(unsilo --version 2>/dev/null || true)"
fi

mkdir -p "$INSTALL_DIR"
# install(1) replaces the file rather than writing through it, so upgrading while
# a copy is running does not corrupt the one in use.
if command -v install >/dev/null 2>&1; then
	install -m 755 "$BINARY" "$INSTALL_DIR/unsilo"
else
	cp "$BINARY" "$INSTALL_DIR/unsilo"
	chmod 755 "$INSTALL_DIR/unsilo"
fi

installed="$("$INSTALL_DIR/unsilo" --version)"
if [ -n "$previous" ] && [ "$previous" != "$installed" ]; then
	printf '  %s -> %s\n' "$previous" "$installed"
else
	printf '  %s\n' "$installed"
fi
printf '  installed to %s\n' "$INSTALL_DIR/unsilo"

case ":$PATH:" in
*":$INSTALL_DIR:"*) ;;
*)
	printf '\n%s is not on your PATH. Add it:\n' "$INSTALL_DIR"
	# $PATH is meant to stay literal here: this line is for the reader to copy.
	# shellcheck disable=SC2016
	printf '  export PATH="%s:$PATH"\n' "$INSTALL_DIR"
	;;
esac

printf '\nStart with: unsilo doctor\nIt writes nothing.\n'
