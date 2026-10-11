#!/bin/sh
# Install omabubble-bridge, then (re)load the service. Run on the Mac, in a
# terminal: the install step asks for your password, because the binary and
# the LaunchAgent plist are root-owned so that code running as you can't swap
# in a replacement and inherit Full Disk Access.
#
#   scripts/mac/install-bridge.sh             build this checkout, sign it, install
#   scripts/mac/install-bridge.sh --release   install the bridge scripts/release.sh
#                                             last built and signed in this copy
set -eu
cd "$(dirname "$0")/../.."

DEST=/usr/local/bin/omabubble-bridge
# Where scripts/release.sh leaves its signed build.
RELEASE_BUILD=target/release/omabubble-bridge-release

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

if [ "${1:-}" = --release ]; then
    [ -f "$RELEASE_BUILD" ] || { echo "no release build here; run scripts/release.sh first" >&2; exit 1; }
    # Installed only if intact and signed with this Mac's identity, so Full
    # Disk Access carries over.
    scripts/mac/sign-bridge.sh --check "$RELEASE_BUILD"
    BIN=$TMP/omabubble-bridge
    cp "$RELEASE_BUILD" "$BIN"
else
    BIN=$TMP/omabubble-bridge
    status=0
    scripts/mac/build-bridge.sh "$BIN" || status=$?
    if [ "$status" = 3 ]; then
        echo "warning: unsigned builds lose Full Disk Access on every update." >&2
    elif [ "$status" != 0 ]; then
        exit "$status"
    fi
fi

# Root-owned binary; `install` replaces the file rather than writing into
# the one launchd is running.
sudo install -d -o root -g wheel -m 0755 "$(dirname "$DEST")"
sudo install -o root -g wheel -m 0755 "$BIN" "$DEST"
sudo "$DEST" install
echo "installed $DEST ($("$DEST" --version))"
