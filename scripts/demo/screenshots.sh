#!/bin/sh
# Regenerate the README screenshots (docs/screenshots/*.webp) from demo data.
# Runs imsg headless in tmux, so nothing appears on screen.
# Needs: sqlite3, tmux, python3 with wcwidth, chromium, ImageMagick.
set -eu
cd "$(dirname "$0")/../.."
ROOT=$PWD
OUT=docs/screenshots
COLS=112
ROWS=30
SOCK=imsg-demo-$$

DIR=$(mktemp -d)
BRIDGE=$(scripts/demo/run.sh --setup "$DIR")
cleanup() {
    tmux -L "$SOCK" kill-server 2>/dev/null || true
    kill "$BRIDGE" 2>/dev/null || true
    [ -n "${KEEP:-}" ] && echo "kept $DIR" || rm -rf "$DIR"
}
trap cleanup EXIT INT TERM

tmux -L "$SOCK" -f /dev/null new-session -d -x $COLS -y $ROWS \
    "XDG_STATE_HOME=$DIR/state $ROOT/target/debug/imsg --config $DIR/config.toml"
tmux -L "$SOCK" set -g status off

keys() { tmux -L "$SOCK" send-keys "$@"; sleep 0.6; }
type_text() { tmux -L "$SOCK" send-keys -l "$1"; sleep 0.6; }

shot() {
    tmux -L "$SOCK" capture-pane -p -e -N > "$DIR/$1.txt"
    python3 scripts/demo/ansi2html.py < "$DIR/$1.txt" > "$DIR/$1.html"
    chromium --headless --disable-gpu --hide-scrollbars --force-device-scale-factor=2 \
        --default-background-color=00000000 --window-size=1300,1000 \
        --screenshot="$DIR/$1.png" "file://$DIR/$1.html" 2>/dev/null
    # Trim the transparent margin but keep room for the shadow.
    # Lossless WebP: text stays crisp and the files are much smaller than PNG.
    magick "$DIR/$1.png" -trim -bordercolor none -border 24 \
        -define webp:lossless=true "$OUT/$1.webp"
    echo "wrote $OUT/$1.webp"
}

mkdir -p "$OUT"
sleep 1.5

# A conversation: Maya's chat open, a reply being typed.
keys Enter
type_text "leaving in 10"
shot chat

# A group chat: senders named from Contacts.
keys Escape
keys j
keys Enter
type_text "8 works, I'll bring water"
shot group
