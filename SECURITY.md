# Security

No app can be proven unhackable. GitKraken, Termius, iTerm2, FileZilla and the others all ship security fixes regularly. This page says how Weave protects your data, what has been checked, and what is still open.

## Reporting a problem

Please email AssayPlot@pm.me instead of opening a public issue. Include the version, your operating system and the steps to reproduce.

## How Weave protects your data

| Area | What Weave does |
| --- | --- |
| Passwords, keys, tokens | Kept in the system credential store (Keychain on macOS, Credential Manager on Windows, Secret Service on Linux), never in Weave's database, logs or settings exports. Passwords you choose not to save stay in memory until Weave quits. |
| Locking the app | Optional lock when Weave opens and after an idle time. macOS uses Touch ID or your login password. Windows and Linux use a Weave lock password, stored only as an Argon2 hash in the credential store. Turning the lock off needs the same proof. While locked, the window is covered and no keystrokes reach terminals. |
| SSH | Passwords, verification codes (Duo, OTP) and new host keys are asked in a Weave dialog and answered only to the `ssh` process that asked, over a private channel with a one-time token: a Unix socket in a folder only you can open, or a loopback port on Windows. A saved password is filled in once and is not retried in a loop. Weave uses the system's OpenSSH, so your keys, agent, `known_hosts` checks and jump hosts work as usual. Host and user names that start with `-` are refused everywhere, including imports, so they cannot be read as `ssh` options. |
| TLS (FTPS, WebDAV, S3, Azure, APIs) | Certificates are checked against the system's trust store. FTPS and WebDAV can also require up to three pinned certificate fingerprints. Plain HTTP is allowed only for localhost. |
| Downloads from servers | File names a server lists are checked before a folder is copied. A name such as `..` or one containing a path separator stops the copy, so a hostile server cannot write outside the destination. Received ZMODEM files keep only their base name and never replace an existing file. |
| Settings vault and team vault | Encrypted with AES-256-GCM, with a key derived from your passphrase using Argon2id. Secrets are never included. |
| The app window | A strict content security policy. Text from servers is shown as text; the only HTML insertion (code highlighting) is escaped. Links in terminal output open only if they are `http` or `https`. Release builds have no developer tools. |
| Listening on the network | Off by default. The TFTP and HTTP file servers listen only on this computer unless you choose otherwise, serve only the chosen folder (path traversal is refused and tested), and the HTTP server is read-only. Sign-in and VNC helpers listen on 127.0.0.1 only, and the VNC bridge also needs a one-time token. |
| AI features | Nothing is sent to Anthropic unless you click Write message, Explain or Write pull request description, using your own key. |
| Dependencies | Checked with `npm audit` and `cargo audit`. |

## Review of 19 October 2026

Fixed:

1. Folder downloads could escape the destination. A malicious FTP, S3 or Azure server could list an entry named `..` and make a folder download write elsewhere. Folder listings are now checked, with a test.
2. Cryptomator vault names had the same issue. Anyone sharing a vault could name an entry `..`. Such entries are now skipped.
3. The TLS library had two published advisories (rustls 0.23.45, RUSTSEC-2026-0348 and RUSTSEC-2026-0349). It was updated to 0.23.46. An unused QUIC library (quinn-proto) was updated as well.
4. The HTTP file server's folder pages showed the requested path without escaping it. It is now escaped, with a test that uses a hostile folder name.
5. Importing an OpenSSH config skipped host validation. Entries with option-like hosts are now skipped.
6. Terminal hyperlinks (OSC 8) used xterm's default handler. They now open only `http` and `https` addresses.
7. Saved S3 hosts showed the access key ID on the Overview. Storage addresses are now shown without it.

Checked and found sound: every place the interface inserts HTML, URL opening, the VNC bridge token, local listeners, file server path containment, SQL (all statements are parameterised), what is written to logs, and the Git password helper (it answers only for the host it was set up for).

Remaining notes from the dependency audit: `glib` (unsound iterator, RUSTSEC-2024-0429), and `zbus`, `zvariant` and `proc-macro-error` (unmaintained). Tauri pulls these in for Linux builds and at build time.

## Known limits

- The installers are not signed with paid certificates. Notarizing for macOS needs a paid Apple Developer membership, and Windows code signing needs a paid certificate, so the first launch shows a warning on both. Each release publishes SHA-256 checksums, and you can build from source instead.
- Weave is not sandboxed. Like iTerm2, Termius and other terminal and Git apps, it runs outside the system sandbox because it has to run shells, `git` and `ssh`. Anything that can control Weave's window can act as you, so the protections above are what keep untrusted content from doing that.
- Imported settings vaults are trusted. A vault can contain triggers that type into terminals, so import only vaults you made or trust.
- Weave has not had an independent security audit. The review above was done by the project itself, and an outside review is welcome.
