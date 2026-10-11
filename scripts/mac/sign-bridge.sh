#!/bin/sh
# Sign an omabubble-bridge binary with the local signing identity (see
# make-signing-cert.sh). Signing every build with the same identity makes
# macOS treat new versions as the same app, so Full Disk Access survives.
#
#   scripts/mac/sign-bridge.sh <binary>           sign it
#   scripts/mac/sign-bridge.sh --check <binary>   verify it's intact and was
#                                                 signed with this identity
#
# Exits 3 when there's no signing identity, so callers can decide whether an
# unsigned build is acceptable.
set -eu

CHECK=
if [ "${1:-}" = --check ]; then
    CHECK=1
    shift
fi
BIN=$1
DIR="$HOME/.config/omabubble-signing"
KC="$DIR/signing.keychain-db"

if [ ! -f "$KC" ]; then
    echo "no signing identity (run scripts/mac/make-signing-cert.sh)" >&2
    exit 3
fi

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
# The certificate is self-signed (untrusted), so sign by fingerprint, not
# name. The keychain holds only this one certificate.
HASH=$(security find-certificate -a -Z "$KC" | awk '/SHA-1 hash/ {print $3; exit}')
[ -n "$HASH" ] || { echo "no certificate in $KC" >&2; exit 3; }

if [ -n "$CHECK" ]; then
    codesign --verify --strict "$BIN"
    # The designated requirement names the signing certificate's SHA-1:
    #   identifier "com.omabubble.bridge" and certificate leaf = H"…"
    leaf=$(codesign -d -r- "$BIN" 2>&1 | sed -n 's/.*certificate leaf = H"\([0-9a-fA-F]*\)".*/\1/p')
    if [ "$(echo "$leaf" | tr a-f A-F)" != "$(echo "$HASH" | tr a-f A-F)" ]; then
        echo "$BIN is not signed with this Mac's omabubble-bridge identity" >&2
        exit 1
    fi
    exit 0
fi

codesign --force --sign "$HASH" --keychain "$KC" --identifier com.omabubble.bridge "$BIN"
