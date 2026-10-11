#!/bin/sh
# Keep macOS's clock right when it runs in a virtual machine (for example
# Parallels or UTM on a Mac).
#
# A VM's clock stops while its host sleeps, and macOS doesn't catch up on its
# own afterwards (seen here: 15 hours behind after a night's suspend). Apple's
# services then misbehave and messages arrive late. This installs a root
# LaunchDaemon that sets the clock from time.apple.com every 2 minutes.
#
#   scripts/mac/install-timesync.sh              install (asks for your password)
#   scripts/mac/install-timesync.sh --uninstall  remove
set -eu

LABEL=com.omabubble.timesync
PLIST=/Library/LaunchDaemons/$LABEL.plist
# Its name before the rename to omabubble (0.2.0).
OLD_LABEL=com.imsg.timesync

remove_old() {
    if [ -f "/Library/LaunchDaemons/$OLD_LABEL.plist" ]; then
        sudo launchctl bootout "system/$OLD_LABEL" 2>/dev/null || true
        sudo rm -f "/Library/LaunchDaemons/$OLD_LABEL.plist"
        echo "removed $OLD_LABEL"
    fi
}

if [ "${1:-}" = --uninstall ]; then
    sudo launchctl bootout "system/$LABEL" 2>/dev/null || true
    sudo rm -f "$PLIST"
    echo "removed $LABEL"
    remove_old
    exit 0
fi

TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT
cat >"$TMP" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/sntp</string>
        <string>-sS</string>
        <string>time.apple.com</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StartInterval</key>
    <integer>120</integer>
    <key>StandardOutPath</key>
    <string>/dev/null</string>
    <key>StandardErrorPath</key>
    <string>/dev/null</string>
</dict>
</plist>
EOF

sudo launchctl bootout "system/$LABEL" 2>/dev/null || true
sudo install -o root -g wheel -m 0644 "$TMP" "$PLIST"
sudo launchctl bootstrap system "$PLIST"
remove_old
echo "installed $LABEL: sets the clock from time.apple.com every 2 minutes"
