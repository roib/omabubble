#!/bin/sh
# Build, sign and install imsg-bridge, then (re)load the service.
# Run on the Mac, in a terminal: the install step asks for your password,
# because the binary and the LaunchAgent plist are root-owned so that code
# running as you can't swap in a replacement and inherit Full Disk Access.
set -eu
cd "$(dirname "$0")/../.."

NAME="imsg-bridge local signing"
DIR="$HOME/.config/imsg-signing"
KC="$DIR/signing.keychain-db"
MISE=${MISE:-/opt/local/bin/mise}
DEST=/usr/local/bin/imsg-bridge

"$MISE" exec -- cargo build --release -p bridge
BIN=target/release/imsg-bridge

if [ -f "$KC" ]; then
    # Older setups stored a random keychain password in a file, which then
    # showed up in `ps` on every unlock. Switch the keychain to an empty
    # password once; the 0600 file mode is the protection either way.
    if [ -f "$DIR/keychain-password" ]; then
        security set-keychain-password -o "$(cat "$DIR/keychain-password")" -p "" "$KC"
        rm -f "$DIR/keychain-password"
    fi
    security unlock-keychain -p "" "$KC"
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

# Root-owned binary; `install` replaces the file rather than writing into
# the one launchd is running.
sudo install -d -o root -g wheel -m 0755 "$(dirname "$DEST")"
sudo install -o root -g wheel -m 0755 "$BIN" "$DEST"
sudo "$DEST" install
echo "installed $DEST ($("$DEST" --version))"

OLD="$HOME/.local/bin/imsg-bridge"
if [ -f "$OLD" ]; then
    rm -f "$OLD"
    echo "removed old user-owned copy $OLD"
    echo "Update ~/.ssh/authorized_keys to: restrict,command=\"$DEST connect\" …"
fi
