#!/bin/sh
# Regenerate the README screenshots (docs/screenshots/*.webp) from demo data.
# Runs omabubble headless in tmux, so nothing appears on screen.
# Needs: sqlite3, tmux, python3 with wcwidth, chromium, ImageMagick.
set -eu
cd "$(dirname "$0")/../.."
ROOT=$PWD
OUT=docs/screenshots
COLS=112
ROWS=30
SOCK=omabubble-demo-$$

DIR=$(mktemp -d)
BRIDGE=$(scripts/demo/run.sh --setup "$DIR")
cleanup() {
    tmux -L "$SOCK" kill-server 2>/dev/null || true
    kill "$BRIDGE" 2>/dev/null || true
    [ -n "${KEEP:-}" ] && echo "kept $DIR" || rm -rf "$DIR"
}
trap cleanup EXIT INT TERM

tmux -L "$SOCK" -f /dev/null new-session -d -x $COLS -y $ROWS \
    "XDG_STATE_HOME=$DIR/state XDG_CONFIG_HOME=$DIR/xdg-config $ROOT/target/debug/omabubble --config $DIR/config.toml"
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

# Keep drafts out of the shots: a draft adds a ✏ to the chat list, which tmux
# and the TUI measure at different widths, so the borders would shift.

# A picture in a conversation, shown as a thumbnail (half-blocks here; real
# terminals with a graphics protocol show it at full resolution).
keys j j j j
keys Enter
shot picture

# The same picture in the full-pane viewer.
keys Escape
keys Tab
keys k
keys k
keys Enter
shot viewer

# A conversation: Maya's chat open, a reply being typed.
keys Escape
keys Escape
keys k k k k
keys Enter
type_text "leaving in 10"
shot chat

# A group chat: senders named from Contacts. Clear the reply first so it
# doesn't stay behind as a draft.
keys C-u
keys Escape
keys j
keys Enter
type_text "8 works, I'll bring water"
shot group
