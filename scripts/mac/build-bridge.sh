#!/bin/sh
# Build omabubble-bridge from this checkout and sign it (see sign-bridge.sh).
#
#   scripts/mac/build-bridge.sh <output>
#
# Exits 3 when the build worked but there's no signing identity, so callers
# can decide whether an unsigned build is acceptable.
set -eu
cd "$(dirname "$0")/../.."
OUT=$1

# ssh sessions have a short PATH, so also look where mise usually lives:
# Homebrew (Apple silicon, Intel), MacPorts, and mise's own installer.
MISE=${MISE:-$(command -v mise || true)}
for m in /opt/homebrew/bin/mise /usr/local/bin/mise /opt/local/bin/mise "$HOME/.local/bin/mise"; do
    [ -z "$MISE" ] || break
    [ ! -x "$m" ] || MISE=$m
done
[ -n "$MISE" ] || { echo "mise not found: install it, or set MISE to its path" >&2; exit 1; }

MISE_QUIET=1 "$MISE" exec -- cargo build --release -q -p bridge
cp target/release/omabubble-bridge "$OUT"
scripts/mac/sign-bridge.sh "$OUT"
