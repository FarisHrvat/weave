# Privacy

Last updated: 20 November 2026

This notice covers the Weave desktop app and its website at farishrvat.github.io/weave. It is written with the EU General Data Protection Regulation (GDPR) in mind.

## The short version

Weave has no accounts, no telemetry, no analytics, no crash reporting and no automatic updates. Nothing you do in the app is sent to me. Your hosts, settings and history stay on your computer, and your passwords stay in your system's credential store.

## What the app stores on your computer

| What | Where |
| --- | --- |
| Hosts, connections, sync jobs, the transfer queue and history, settings | `weave.db`, a SQLite file in Weave's data folder |
| Passwords, SSH key passphrases you chose to save, API keys and sign-in tokens | The system credential store: Keychain on macOS, Credential Manager on Windows, Secret Service (such as GNOME Keyring) on Linux |
| Snippets, terminal command history, bookmarks, layout choices | The app's web view storage |
| Exported client certificates and copies of files you opened in another app | Weave's data folder, readable only by your user account |

Weave's data folder is:

- macOS: `~/Library/Application Support/app.weave.desktop`
- Windows: `%APPDATA%\app.weave.desktop`
- Linux: `~/.local/share/app.weave.desktop`

Terminal output is not saved unless you start a recording. Recordings and exported settings vaults are saved only where you choose.

## What the app sends, and to whom

Weave connects only to places you set up or ask it to use:

- the servers in your hosts and connections (SSH, FTP, WebDAV, S3, Azure and so on);
- Git remotes, and the GitHub, GitLab, Bitbucket, Azure DevOps or Jira servers you add;
- Dropbox, Google Drive or OneDrive, after you sign in with your own app registration;
- Anthropic's API, only when you press Write message, Explain or Write pull request description. That request contains the staged changes or commit you asked about, and it uses your own API key. Anthropic's own privacy terms apply to it.

Each of those services handles your data under its own terms. Weave adds nothing of its own to these requests: no tracking IDs, no usage data.

## Removing your data

Deleting a host in Weave removes its saved password and sign-ins, settings, tunnels, sync jobs, queued transfers, terminal history, snippets and bookmarks.

To remove everything, uninstall Weave and delete its data folder (above). Saved passwords live in the credential store under the service name `app.weave.desktop`, and you can delete them there too.

## The website

The website is a static page hosted on GitHub Pages. It uses no cookies, no analytics and no third-party scripts or fonts. GitHub, as the host, receives standard request data such as your IP address in its server logs. See the [GitHub Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement). Downloads come from GitHub Releases under the same terms.

## Your rights

Because the app sends me no personal data, I hold none about you from it. If you email me, I use your address and message only to reply. You can ask what I hold about you, or ask me to delete it, at any time. You can also complain to your local data protection authority.

## Contact

Faris Hrvat, AssayPlot@pm.me
