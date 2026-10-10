# Changelog

imsg and imsg-bridge are released together under one version. A new minor
version (0.2 → 0.3) adds features or changes the bridge protocol; a patch
version (0.2.0 → 0.2.1) only fixes things. The protocol version is listed
when it changes: a TUI and a bridge must speak the same one.

## Unreleased

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
