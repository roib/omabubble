#!/bin/sh
# Run imsg against made-up demo data, entirely on this machine: a local
# imsg-bridge serves a demo chat.db (sends are dry runs), and imsg connects to
# it with its own config and state, so your real setup is never touched.
#
#   scripts/demo/run.sh            build, start the bridge, run imsg
#   scripts/demo/run.sh --setup D  only build into D and start the bridge
#                                  (prints its PID; used by screenshots.sh)
set -eu
cd "$(dirname "$0")/../.."
ROOT=$PWD
FIX=bridge/tests/fixtures

setup() {
    dir=$1
    rm -rf "$dir"
    mkdir -p "$dir/contacts/Sources/DEMO" "$dir/state"
    # Schemas come from the test fixtures; only the rows are demo data.
    schema() { awk '/^CREATE/ { p = 1 } p { print } /;[[:space:]]*(--.*)?$/ { p = 0 }' "$1"; }
    { schema "$FIX/make_fixture.sql"; cat scripts/demo/demo.sql; } | sqlite3 "$dir/chat.db"
    { schema "$FIX/make_contacts.sql"; cat scripts/demo/contacts.sql; } |
        sqlite3 "$dir/contacts/Sources/DEMO/AddressBook-v22.abcddb"

    cargo build -q -p bridge -p tui
    cat > "$dir/config.toml" <<EOF
command = ["$ROOT/target/debug/imsg-bridge", "connect", "--socket", "$dir/bridge.sock"]
notifications = false
EOF
    "$ROOT/target/debug/imsg-bridge" serve --db "$dir/chat.db" --socket "$dir/bridge.sock" \
        --contacts "$dir/contacts" --dry-run-send >"$dir/bridge.log" 2>&1 &
    BRIDGE=$!
    i=0
    while [ ! -S "$dir/bridge.sock" ]; do
        i=$((i + 1))
        [ $i -lt 100 ] || { cat "$dir/bridge.log" >&2; exit 1; }
        sleep 0.05
    done
}

if [ "${1:-}" = --setup ]; then
    setup "$2"
    echo "$BRIDGE"
    exit 0
fi

DIR=$(mktemp -d)
setup "$DIR"
trap 'kill $BRIDGE 2>/dev/null; rm -rf "$DIR"' EXIT INT TERM
XDG_STATE_HOME="$DIR/state" "$ROOT/target/debug/imsg" --config "$DIR/config.toml"
