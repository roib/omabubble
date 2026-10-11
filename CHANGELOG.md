# Changelog

omabubble and omabubble-bridge (imsg before 0.2.0) are released together
under one version. A new minor version (0.2 → 0.3) adds features or changes
the bridge protocol; a patch version (0.2.0 → 0.2.1) only fixes things. The
protocol version is listed when it changes: a TUI and a bridge must speak the
same one.

## Unreleased

- The sample config's `remote_command` example points at
  `/usr/local/bin/omabubble-bridge`, where the bridge is installed.
- README: setup in order (Mac, SSH key, Linux) with the steps that were
  missing (Remote Login, the Command Line Tools, the signing identity), and
  screenshots of pictures in a conversation and in the viewer. The demo's
  photos are a reef picture (`scripts/demo/reef.jpg`) and tomatoes drawn by
  `scripts/demo/pictures.py`.
- Releases go to GitHub Releases, with a Linux build of the TUI and its
  checksum. The bridge isn't published, since it must be signed on your own
  Mac; `install-bridge.sh --release` installs the one `release.sh` built and
  signed there. An optional, uncommitted `./local-release.sh` runs after
  publishing.
- `scripts/mac/build-bridge.sh` builds and signs the bridge, finding `mise`
  on the PATH or in the usual Homebrew, MacPorts and installer locations.
- Added `SECURITY.md` and a CI workflow (fmt, clippy, tests).
- The one-time moves from imsg are gone, so omabubble never touches files of
  other programs named imsg. Coming from 0.1.0, install 0.2.0 first, or move
  the folders by hand: `~/.config/imsg` and `~/.local/state/imsg` to
  `omabubble`, and on the Mac `~/Library/Application Support/imsg` to
  `omabubble` and `~/.config/imsg-signing` to `omabubble-signing`; then
  remove the `com.imsg.bridge` and `com.imsg.timesync` jobs.
- `sign-bridge.sh` uses the certificate in `~/.config/omabubble-signing`
  whatever its name.
- `release.sh` keeps its build copy on the Mac in
  `~/Library/Caches/omabubble/release` (was `~/src/appl-tui`); set
  `OMABUBBLE_MAC_DIR` in `local.env` to change it.

## 0.2.0 (2026-10-10)

- **imsg is now omabubble.** The commands are `omabubble` and
  `omabubble-bridge`. The first start moves `~/.config/imsg` and
  `~/.local/state/imsg` to `omabubble`, and the bridge moves its read marks
  the same way. On the Mac, `install-bridge.sh` replaces the `com.imsg.bridge`
  service with `com.omabubble.bridge` and keeps `/usr/local/bin/imsg-bridge`
  as a link, so existing `authorized_keys` lines keep working. Grant Full Disk
  Access to `/usr/local/bin/omabubble-bridge` once (it's a new path), and
  allow Automation again on the first send. `install-timesync.sh` replaces
  `com.imsg.timesync` the same way. Protocol version unchanged (4).
- `release.sh` and `install-bridge.sh` read the Mac and the artifacts site
  from `local.env` (see `local.env.example`) instead of built-in defaults.

## 0.1.0 (2026-10-10)

First release. Protocol version 4.

- Chats and messages from Messages on a Mac, read through `imsg-bridge` over
  SSH; sending as iMessage or SMS.
- One person's chats are merged using the Mac's Contacts, with Contacts
  names shown by default.
- Delivered / Read under your latest message, a delivery marker on every
  sent message, "Sent as Text Message", and late delivery failures.
- Failed messages stay in place: Enter resends, x discards.
- Pictures as thumbnails, with a full-pane viewer.
- Unread counts are kept by the bridge and cleared when chats are read on
  the phone.
- A draft per chat.
- Emoji completion (`:code`) and a Ctrl+E picker.
- Mouse support: click chats and panes, wheel scrolling.
- Desktop notifications while away, clickable to open the chat; hidden
  chats.
- A warning when the Mac's clock is off, and `install-timesync.sh` to keep a
  VM Mac's clock right.
- The bridge runs root-owned and signed, so Full Disk Access survives
  updates.
- `imsg --version` and `imsg-bridge --version` include the git commit the
  build came from, with `-dirty` for builds with uncommitted changes. The
  status bar warns when the bridge is a different release from the TUI.
