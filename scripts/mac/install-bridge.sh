#!/bin/sh
# Build, sign and install imsg-bridge, then restart the service if it's loaded.
# Run on the Mac from anywhere in the repo checkout.
set -eu
cd "$(dirname "$0")/../.."

NAME="imsg-bridge local signing"
DIR="$HOME/.config/imsg-signing"
KC="$DIR/signing.keychain-db"
MISE=${MISE:-/opt/local/bin/mise}
DEST="$HOME/.local/bin/imsg-bridge"
LABEL="gui/$(id -u)/com.imsg.bridge"

"$MISE" exec -- cargo build --release -p bridge
BIN=target/release/imsg-bridge

if [ -f "$KC" ]; then
    security unlock-keychain -p "$(cat "$DIR/keychain-password")" "$KC"
    if ! security list-keychains -d user | grep -qF "$KC"; then
        eval "security list-keychains -d user -s $(security list-keychains -d user | tr '\n' ' ') \"$KC\""
    fi
    # The certificate is self-signed (untrusted), so sign by fingerprint, not name.
    HASH=$(security find-certificate -c "$NAME" -Z "$KC" | awk '/SHA-1 hash/ {print $3}')
    codesign --force --sign "$HASH" --keychain "$KC" --identifier com.imsg.bridge "$BIN"
else
    echo "warning: no signing identity (run scripts/mac/make-signing-cert.sh)." >&2
    echo "         Unsigned builds lose Full Disk Access on every update." >&2
fi

# Replace atomically: never overwrite the binary launchd is running.
mkdir -p "$(dirname "$DEST")"
cp "$BIN" "$DEST.new"
mv -f "$DEST.new" "$DEST"
echo "installed $DEST ($("$DEST" --version))"

if launchctl print "$LABEL" >/dev/null 2>&1; then
    launchctl kickstart -k "$LABEL"
    echo "restarted com.imsg.bridge"
else
    echo "service not loaded; run: $DEST install"
fi
