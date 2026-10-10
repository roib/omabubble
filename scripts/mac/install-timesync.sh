#!/bin/sh
# Keep the Mac's clock right when it runs as a VM.
#
# A VM's clock stops while its host sleeps, and macOS doesn't catch up on its
# own afterwards (seen here: 15 hours behind after a night's suspend). Apple's
# services then misbehave and messages arrive late. This installs a root
# LaunchDaemon that sets the clock from time.apple.com every 2 minutes.
#
#   scripts/mac/install-timesync.sh              install (asks for your password)
#   scripts/mac/install-timesync.sh --uninstall  remove
set -eu

LABEL=com.imsg.timesync
PLIST=/Library/LaunchDaemons/$LABEL.plist

if [ "${1:-}" = --uninstall ]; then
    sudo launchctl bootout "system/$LABEL" 2>/dev/null || true
    sudo rm -f "$PLIST"
    echo "removed $LABEL"
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
echo "installed $LABEL: sets the clock from time.apple.com every 2 minutes"
