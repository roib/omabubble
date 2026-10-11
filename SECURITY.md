# Security

omabubble-bridge runs with Full Disk Access to your Messages and can send
messages as you, so security problems matter here.

## Reporting a vulnerability

Please report privately through GitHub: the repository's **Security** tab →
**Report a vulnerability**. Don't open a public issue for it.

Include what you found, how to reproduce it, and what an attacker could do
with it. You'll get an answer within a few days.

## Supported versions

Only the latest release is fixed.

## Scope

Of most interest:

- anything that lets code other than the installed, root-owned bridge read
  Messages or Contacts data, or send messages
- ways around the SSH forced command, or for one SSH key to do more than
  relay to the bridge
- message content, phone numbers or email addresses leaking into logs,
  files or other machines

The design is described in the README's Security section.
