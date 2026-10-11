#!/bin/sh
# One-time: create a self-signed code-signing identity for omabubble-bridge.
#
# Signing every build with the same identity makes macOS treat rebuilds as the
# same app, so Full Disk Access and Automation permissions survive updates.
# The identity lives in its own keychain with an empty password: its only
# protection is the 0600 file mode, which is also all a stored password would
# give, and it keeps the password out of `ps` when scripts unlock it.
set -eu

NAME="omabubble-bridge local signing"
DIR="$HOME/.config/omabubble-signing"
KC="$DIR/signing.keychain-db"

# An identity made before the rename (0.2.0) is still used; see sign-bridge.sh.
for kc in "$KC" "$HOME/.config/imsg-signing/signing.keychain-db"; do
    if [ -f "$kc" ]; then
        echo "already set up: $kc"
        exit 0
    fi
done

mkdir -p "$DIR"
chmod 700 "$DIR"
umask 077

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
    -out "$TMP/identity.p12" -passout pass:

security create-keychain -p "" "$KC"
security set-keychain-settings "$KC" # no auto-lock
security unlock-keychain -p "" "$KC"
security import "$TMP/identity.p12" -k "$KC" -P "" -T /usr/bin/codesign >/dev/null
# Let codesign use the key without a GUI prompt.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "" "$KC" >/dev/null
# codesign only finds private keys in keychains on the search list; append ours.
eval "security list-keychains -d user -s $(security list-keychains -d user | tr '\n' ' ') \"$KC\""

echo "created \"$NAME\" in $KC (valid 10 years)"
