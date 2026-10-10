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

# Two made-up photos (needs ImageMagick; without it they show as labels).
# The bridge serves pictures only from beside chat.db, so they go there.
pictures() {
    command -v magick >/dev/null || return 0
    a="$1/Attachments/00"
    mkdir -p "$a/A1" "$a/A2"
    # Dad's tomatoes: a garden bed with ripe fruit.
    magick -size 1200x900 gradient:'#9bd36a'-'#2e5a24' \
        -fill '#5a3a22' -draw 'rectangle 0,700 1200,900' \
        -fill '#d8322a' -draw 'circle 330,420 405,420' -draw 'circle 560,360 630,360' \
        -draw 'circle 790,450 868,450' -draw 'circle 470,560 535,560' -draw 'circle 700,600 770,600' \
        -fill '#3f7d2c' -draw 'circle 330,350 350,350' -draw 'circle 560,292 578,292' \
        -draw 'circle 790,377 810,377' -quality 85 "$a/A1/IMG_2041.jpg"
    # Jordan's lake: sky, mountains, water.
    magick -size 1200x800 gradient:'#bfe3ff'-'#f6e3c8' \
        -fill '#5b6f86' -draw 'polygon 0,520 260,250 470,470 700,200 980,480 1200,330 1200,560 0,560' \
        -fill '#2f6e9e' -draw 'rectangle 0,540 1200,800' \
        -fill '#ffffff' -draw 'polygon 700,200 650,260 750,260' -quality 85 "$a/A2/IMG_3310.jpg"
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
