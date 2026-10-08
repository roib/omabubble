# imsg

iMessage in a terminal on a non-Apple machine. A Mac that stays signed in to
Messages runs **`imsg-bridge`**. The Linux TUI **`imsg`** talks to it over SSH.
Contacts aren't looked up: chats show bare phone numbers and emails.

```
imsg (Linux) ──ssh, restricted key──> imsg-bridge connect ──unix socket──> imsg-bridge serve (LaunchAgent)
                                                                             ├─ reads ~/Library/Messages/chat.db (read-only)
                                                                             ├─ watches it with FSEvents, pushes new/edited/unsent messages
                                                                             └─ sends through Messages.app with osascript
```

The only network port is SSH. Full Disk Access is granted to `imsg-bridge`
alone, not to sshd.

## Build

The Rust version is pinned in `mise.toml`, and both machines use it via mise.

```sh
# Linux: the TUI
mise exec -- cargo build --release -p tui          # → target/release/imsg

# Mac, once: a self-signed code-signing identity (its own keychain, no prompts)
~/src/appl-tui/scripts/mac/make-signing-cert.sh

# Mac, every update: build, sign, install to ~/.local/bin, restart the service
~/src/appl-tui/scripts/mac/install-bridge.sh
```

Signing every build with the same identity makes macOS treat rebuilds as the
same app, so Full Disk Access and Automation survive updates. Unsigned builds
lose both permissions every time the binary changes.

## Mac setup (once)

1. **Full Disk Access:** go to System Settings → Privacy & Security → Full
   Disk Access, click **+**, and add `/Users/you/.local/bin/imsg-bridge`.
   Press ⌘⇧. in the file picker to show hidden folders.
2. **Start the service:** `~/.local/bin/imsg-bridge install`. This writes
   `~/Library/LaunchAgents/com.imsg.bridge.plist` and loads it. Logs go to
   `~/Library/Logs/imsg-bridge.log`.
3. **Automation permission:** the first real send makes macOS ask whether
   `imsg-bridge` may control Messages. Click Allow. If you miss the prompt,
   sends fail with `-1743`; turn it on under Privacy & Security → Automation.
4. **Stay reachable:**
   - Keep the Mac logged in, with Messages running.
   - Turn on "Prevent automatic sleeping" (Energy settings) or run `caffeinate`.
   - For SMS (green bubble) contacts, turn on Text Message Forwarding to this Mac on the iPhone.
5. **Restricted SSH key:** this key can only reach the bridge.
   - On Linux: `ssh-keygen -t ed25519 -f ~/.ssh/imsg -N ''`
   - On the Mac, add one line to `~/.ssh/authorized_keys`:
     ```
     command="/Users/you/.local/bin/imsg-bridge connect",no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding ssh-ed25519 AAAA…(contents of ~/.ssh/imsg.pub)
     ```
   - On Linux, add to `~/.ssh/config`:
     ```
     Host mac-imsg
         HostName 127.0.0.1
         Port 2233
         User b
         IdentityFile ~/.ssh/imsg
         IdentitiesOnly yes
     ```

   For use away from home, use Tailscale and the Mac's Tailscale name rather
   than forwarding port 22 on your router.

To remove the service, run `imsg-bridge uninstall`.

**Re-granting permissions:** this is only needed if the binary's identity
changes, for example after switching from unsigned to signed or recreating the
certificate. Remove `imsg-bridge` from the Full Disk Access list and add it
again, then allow the Automation prompt on the next send. Over VNC, this opens
both windows for you:
`open -R ~/.local/bin/imsg-bridge; open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"`

## Linux setup

`~/.config/imsg/config.toml` (running `imsg` without one prints this sample):

```toml
host = "mac-imsg"
country_code = "1"      # prepended to 10-digit numbers in the new-conversation popup
notifications = true    # notify-send for messages in chats you don't have open
bell = false
# ssh_args = ["-p", "2233"]                                   # if the host alias doesn't cover it
# remote_command = "/Users/you/.local/bin/imsg-bridge connect"  # only for keys without a forced command
```

Then run `imsg`, or `imsg <host>` to override the configured host.

### One person, several handles

`chat.db` keeps a separate chat row per handle, so a contact you message by
both phone number and email has two rows, and Messages may file your sent copy
under either. Messages.app shows them as one conversation by consulting
Contacts, and the bridge does the same: it reads each card's phone numbers,
email addresses and link id from `~/Library/Application Support/AddressBook/`
(covered by the Full Disk Access grant), matches them against the handles in
`chat.db`, and tells the TUI which handles belong to one person. Only those
groupings cross the wire; names are included only with `names = true`.

For someone Contacts doesn't link, add an override:

```toml
[[person]]
name = "Alex"            # optional: shown instead of the handles
handles = ["+15551234567", "alex@example.com"]
```

If a sent copy still lands in a sibling chat that isn't merged, the TUI counts
it as sent and shows a hint naming the two handles. `imsg-bridge serve
--no-contacts` turns the Contacts lookup off, and `imsg-bridge query people`
prints only counts.

## Keys

| Where | Key | Action |
|---|---|---|
| Chat list | `j`/`k`, arrows, `g`/`G` | Move |
| | `Enter` | Open chat |
| | `n` | New conversation (number or email) |
| Anywhere | `Tab` / `Shift-Tab` | Cycle focus: list → messages → input |
| | `PgUp` / `PgDn` | Scroll; at the top, older history loads |
| | `q` (outside the input), `Ctrl-C` | Quit |
| | `r` | Reconnect now, when disconnected |
| Input | `Enter` | Send |
| | `Alt-Enter` | Newline |
| | `Ctrl-W` / `Ctrl-U` | Delete word / clear |
| | `Esc` | Back to the list |
| Messages pane | `j`/`k` | Select a message |
| | `y` | Copy the selected message |
| | `Y` | Copy with `[time sender]` |
| | `v` | Start or cancel a range; `y`/`Y` then copy the range |
| | `Esc` | Cancel range / back |

Pasting uses bracketed paste, so multi-line text lands in the input box and
isn't sent until you press Enter. Copying writes OSC 52 (works over SSH, and in
tmux with `set-clipboard on`), and also uses `wl-copy`/`xclip` when available.
The app doesn't capture the mouse, so your terminal's drag-select still works.

## Send status

| Shown | Meaning |
|---|---|
| `sending…` | Handed to the bridge, waiting for the message to appear in chat.db |
| time | Confirmed: the message is in chat.db |
| `✗ failed` | osascript failed, or Messages marked it with an error |
| `? unconfirmed` | No echo after 30s, or the connection dropped mid-send |

## Limitations

- Edits and unsends from other people (or your phone) show up, but you can't
  edit or unsend from the TUI: AppleScript has no command for either.
- Reading a chat here doesn't mark it read on the Mac or phone.
- Delivered/read receipts, typing indicators, sending attachments and tapbacks
  aren't supported.
- New messages only arrive while the Mac is awake and online. After a
  reconnect, the TUI catches up on everything it missed.

## Development

Everything can run against a fake database, without touching real messages:

```sh
sqlite3 /tmp/fx.db < bridge/tests/fixtures/make_fixture.sql
cargo run -p bridge -- serve --db /tmp/fx.db --socket /tmp/imsg/b.sock --dry-run-send
# TUI config for local testing:
#   command = ["/path/to/target/debug/imsg-bridge", "connect", "--socket", "/tmp/imsg/b.sock"]
cargo run -p tui -- --config /tmp/imsg-dev.toml
```

- `--dry-run-send` logs the exact osascript argv instead of running it.
- `imsg-bridge query --db <path> chats|history <id>|since <rowid>` prints raw JSON.
- `cargo test --workspace` covers:
  - DB queries, attributedBody decoding and change detection
  - the socket server
  - send/echo matching, reconnect catch-up and copy ranges
  - phone normalization and emoji-safe wrapping

Protocol: newline-delimited JSON, defined in `proto/src/lib.rs`. The bridge
sends `hello` with a protocol version first. On a mismatch, the TUI refuses to
continue and asks you to rebuild.
