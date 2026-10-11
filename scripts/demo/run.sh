#!/bin/sh
# Run omabubble against made-up demo data, entirely on this machine: a local
# omabubble-bridge serves a demo chat.db (sends are dry runs), and omabubble connects to
# it with its own config and state, so your real setup is never touched.
#
#   scripts/demo/run.sh            build, start the bridge, run omabubble
#   scripts/demo/run.sh --setup D  only build into D and start the bridge
#                                  (prints its PID; used by screenshots.sh)
set -eu
cd "$(dirname "$0")/../.."
ROOT=$PWD
FIX=bridge/tests/fixtures

# Two photos (needs ImageMagick; without it they show as labels): Dad's
# tomatoes, drawn by pictures.py, and Jordan's reef, scripts/demo/reef.jpg.
# The bridge serves pictures only from beside chat.db, so they go there.
pictures() {
    command -v magick >/dev/null || return 0
    a="$1/Attachments/00"
    mkdir -p "$a/A1" "$a/A2"
    python3 -I scripts/demo/pictures.py "$a/A1/IMG_2041.jpg"
    cp scripts/demo/reef.jpg "$a/A2/IMG_3310.jpg"
    sqlite3 "$1/chat.db" "
        UPDATE attachment SET filename = '$a/A1/IMG_2041.jpg', mime_type = 'image/jpeg',
            transfer_name = 'IMG_2041.jpg' WHERE ROWID = 1;
        INSERT INTO attachment (ROWID, guid, filename, mime_type, transfer_name, total_bytes)
            VALUES (2, 'A2', '$a/A2/IMG_3310.jpg', 'image/jpeg', 'IMG_3310.jpg', 0);
        INSERT INTO message_attachment_join VALUES (30, 2);
        UPDATE message SET cache_has_attachments = 1 WHERE ROWID = 30;"
}

setup() {
    dir=$1
    rm -rf "$dir"
    mkdir -p "$dir/contacts/Sources/DEMO" "$dir/state"
    # Schemas come from the test fixtures; only the rows are demo data.
    schema() { awk '/^CREATE/ { p = 1 } p { print } /;[[:space:]]*(--.*)?$/ { p = 0 }' "$1"; }
    { schema "$FIX/make_fixture.sql"; cat scripts/demo/demo.sql; } | sqlite3 "$dir/chat.db"
    { schema "$FIX/make_contacts.sql"; cat scripts/demo/contacts.sql; } |
        sqlite3 "$dir/contacts/Sources/DEMO/AddressBook-v22.abcddb"
    pictures "$dir"

    cargo build -q -p bridge -p tui
    cat > "$dir/config.toml" <<EOF
command = ["$ROOT/target/debug/omabubble-bridge", "connect", "--socket", "$dir/bridge.sock"]
notifications = false
EOF
    "$ROOT/target/debug/omabubble-bridge" serve --db "$dir/chat.db" --socket "$dir/bridge.sock" \
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
XDG_STATE_HOME="$DIR/state" XDG_CONFIG_HOME="$DIR/xdg-config" "$ROOT/target/debug/omabubble" --config "$DIR/config.toml"
