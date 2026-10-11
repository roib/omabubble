#!/bin/sh
# Install omabubble-bridge, then (re)load the service. Run on the Mac, in a
# terminal: the install step asks for your password, because the binary and
# the LaunchAgent plist are root-owned so that code running as you can't swap
# in a replacement and inherit Full Disk Access.
#
#   scripts/mac/install-bridge.sh                    build this checkout, sign it, install
#   scripts/mac/install-bridge.sh --release [VER]    install a published release
#                                                    (default: the latest), e.g. v0.1.0
#
# Releases come from $ARTIFACTS_URL/omabubble-bridge/ (set in local.env) and are
# installed only if their checksum matches and they were signed with this
# Mac's signing identity (scripts/release.sh builds and signs them here).
set -eu
cd "$(dirname "$0")/../.."
[ ! -f local.env ] || . ./local.env

MISE=${MISE:-/opt/local/bin/mise}
BASE=${ARTIFACTS_URL:-https://artifacts.example.com}
DEST=/usr/local/bin/omabubble-bridge

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

if [ "${1:-}" = --release ]; then
    version=${2:-latest}
    case $(uname -m) in
        x86_64) arch=amd64 ;;
        arm64) arch=arm64 ;;
        *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
    esac
    file=omabubble-bridge-darwin-$arch
    curl -fsSL -o "$TMP/$file" "$BASE/omabubble-bridge/$version/$file" \
        || { echo "no $file for omabubble-bridge $version" >&2; exit 1; }
    curl -fsSL -o "$TMP/SHA256SUMS" "$BASE/omabubble-bridge/$version/SHA256SUMS"
    (cd "$TMP" && grep " $file\$" SHA256SUMS | shasum -a 256 -c -)
    scripts/mac/sign-bridge.sh --check "$TMP/$file"
    BIN=$TMP/$file
else
    "$MISE" exec -- cargo build --release -p bridge
    BIN=$TMP/omabubble-bridge
    cp target/release/omabubble-bridge "$BIN"
    status=0
    scripts/mac/sign-bridge.sh "$BIN" || status=$?
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
