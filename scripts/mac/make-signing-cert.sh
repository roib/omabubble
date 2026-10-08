#!/bin/sh
# One-time: create a self-signed code-signing identity for imsg-bridge.
#
# Signing every build with the same identity makes macOS treat rebuilds as the
# same app, so Full Disk Access and Automation permissions survive updates.
# The identity lives in its own keychain with a generated password, so it can
# be unlocked over SSH without touching the login keychain.
set -eu

NAME="imsg-bridge local signing"
DIR="$HOME/.config/imsg-signing"
KC="$DIR/signing.keychain-db"

if [ -f "$KC" ]; then
    echo "already set up: $KC"
    exit 0
fi

mkdir -p "$DIR"
chmod 700 "$DIR"
umask 077
openssl rand -hex 24 >"$DIR/keychain-password"
PW=$(cat "$DIR/keychain-password")

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cat >"$TMP/cert.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF

openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$TMP/cert.cnf" \
    -keyout "$TMP/key.pem" -out "$TMP/cert.pem" 2>/dev/null
openssl pkcs12 -export -inkey "$TMP/key.pem" -in "$TMP/cert.pem" -name "$NAME" \
    -out "$TMP/identity.p12" -passout pass:"$PW"

security create-keychain -p "$PW" "$KC"
security set-keychain-settings "$KC" # no auto-lock
security unlock-keychain -p "$PW" "$KC"
security import "$TMP/identity.p12" -k "$KC" -P "$PW" -T /usr/bin/codesign >/dev/null
# Let codesign use the key without a GUI prompt.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$PW" "$KC" >/dev/null
# codesign only finds private keys in keychains on the search list; append ours.
eval "security list-keychains -d user -s $(security list-keychains -d user | tr '\n' ' ') \"$KC\""

echo "created \"$NAME\" in $KC (valid 10 years)"
