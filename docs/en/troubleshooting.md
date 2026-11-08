# Troubleshooting

Read this page before opening an issue. Everything here is either "looks broken, is by design" or
"one command away from working".

- [macOS refuses to open the app](#macos-refuses-to-open-the-app)
- [Where the API key lives, and why](#where-the-api-key-lives-and-why)
- [The key has to be entered once after upgrading](#the-key-has-to-be-entered-once-after-upgrading)
- [The first screen says "not installed"](#the-first-screen-says-not-installed)
- [Provider errors: 401 / 404 / timeout](#provider-errors-401--404--timeout)
- [The thread is not in sync with the IDE plugins](#the-thread-is-not-in-sync-with-the-ide-plugins)
- [Closing the window does not quit](#closing-the-window-does-not-quit)

---

## macOS refuses to open the app

**What you see.** Double-clicking the app, or the mounted dmg, produces

> "Warden" is damaged and can't be opened. You should move it to the Trash.

or

> "Warden" Not Opened. Apple cannot check it for malicious software.

**Why.** Gatekeeper only lets through two kinds of software: apps from the App Store, or apps signed with an
Apple Developer ID **and notarised**. Warden's release build is neither. This repository holds no Developer ID
certificate, and `release.yml` ad-hoc signs only (`codesign -s -`, whose subject is the binary itself). On top
of that, the browser marks the downloaded dmg with `com.apple.quarantine`, so the first launch hits the dialog.

**What to do** (either one, once):

```bash
# after dragging the app into /Applications
xattr -dr com.apple.quarantine "/Applications/Warden.app"
```

or **right-click → Open** in Finder, then **Open** again in the dialog.

> **Do not disable Gatekeeper.** `sudo spctl --master-disable` switches off download checking for the whole
> machine — one app's problem traded for every app's protection. The command above touches one bundle.

**What would actually fix it.** Apple Developer Program ($99/year) → a Developer ID Application certificate →
`codesign --options runtime` → `xcrun notarytool submit --wait` → `xcrun stapler staple`. After that the app
opens on a double click with no dialog at all. `release.yml` already carries those steps, gated so that they
are skipped when no certificate is configured (see the comments in the workflow). It is not laziness that they
are off: a certificate costs money and requires a verified identity, which is the maintainer's call.

Windows behaves the same way without a code-signing certificate: SmartScreen blocks once, **More info → Run
anyway** gets through.

---

## Where the API key lives, and why

**What you saw** (in v0.1.6 and earlier), on every launch:

> Warden wants to use confidential information stored in "repotory-agent" in your keychain. To allow this,
> enter the login keychain password.

Choosing "Always Allow" did not help — it asked again next time.

**Why.** A keychain item's access control list is bound to the **code signature of the app that created it**.
Warden is ad-hoc signed, and an ad-hoc signature is a hash of that exact binary — so **every new build is a
brand new application as far as the keychain is concerned**, and "always allow" has nothing stable to attach
to.

A credential store that asks for your account password on every launch is not protecting the secret; it is
training you to dismiss the dialog. Since v0.1.7 the provider API keys live in `secrets.json` in the app config
directory, written **`0600` at creation** (not created and then chmod-ed, so there is no readable window).

**The trade-off, stated plainly.** A key in a file is a key in your backups, your sync folders and your screen
shares. What you get for it: no authorization prompt to read it, `cat` and `rm` work, and it is protected by
the same layer as the rest of the app's state. If no key on disk is acceptable, use the local Ollama — it needs
none.

Both files sit in the same directory:

```
macOS    ~/Library/Application Support/com.jiangcheng.repotoryagent/
Windows  %APPDATA%\com.jiangcheng.repotoryagent\
Linux    ~/.config/com.jiangcheng.repotoryagent/
```

- `providers.json` — names, base URLs, default models, extra headers; **no keys**, only whether one exists
- `secrets.json` — `{"provider-id": "sk-..."}`, `0600`

The old keychain entry is still there (macOS does not clean it up on its own). To remove it: open **Keychain
Access**, search `repotory-agent`, delete. Warden no longer reads or writes it.

---

## The key has to be entered once after upgrading

**What you see.** Coming from v0.1.6 or earlier, the first message fails with

> the provider's API key is missing — open the provider settings and save it again

**Why.** The old version kept the key in the system credential store; the new one does not read it (see above).
No migration was written, because **reading the old entry once means showing that password dialog once** — the
only way not to ask is not to read.

**What to do.** Settings → that provider → paste the API key again → save. Once. It will not ask again.

---

## The first screen says "not installed"

The memory lane needs one binary: `repotory-mcp`. It comes from the
[Cinder](https://github.com/sky-jiangcheng/cinder) releases (one asset per platform, desktop app not required)
or from Cinder Desktop's installer. Cinder 2.33 and later also ship the same server as `cinder-mcp`.

```bash
which repotory-mcp          # is it on PATH?
repotory-mcp --version      # does it run?
```

The first screen distinguishes three states — do not read them as one:

| State | Meaning | What to do |
| :--- | :--- | :--- |
| not installed | the binary is not on `PATH` | install it, or add its directory to `PATH` |
| installed but not answering | found, but the handshake failed or timed out | press retry; run it in a terminal and read what it says |
| connected + N tools | working | — |

**A GUI app's `PATH` is not your terminal's.** A process launched from Finder or the Dock does not read your
`.zshrc`, so "`which` finds it, Warden does not" is the usual shape of this: put the binary somewhere system-wide
like `/usr/local/bin` or `~/.local/bin`, not in a directory only an interactive shell loads.

---

## Provider errors: 401 / 404 / timeout

| Symptom | Most likely | Fix |
| :--- | :--- | :--- |
| `404 Not Found` | the base URL is missing a path segment — you gave the site root, the API wants the API root | make it look like `https://host/v1`, then **Test connection** again |
| `401` / `403` | wrong or expired key, or the key has no access to that model | save the key again; check the model id |
| cannot connect / timeout | a proxy is in the way of a local provider, or the service is not running | Warden dials loopback directly; try `curl http://127.0.0.1:11434/v1/models` first |
| empty model list | the service does not implement `GET /models` | type the model id yourself, then **Test connection** |

**Test connection sends a real chat request**, not a ping — it proves endpoint, key and model in one go. When it
fails, one of those three is wrong, and the table above is the order to check them in.

---

## The thread is not in sync with the IDE plugins

Three separate facts:

- **A conversation with a shared topic syncs**: topics and messages poll `repotory_chat`, and Warden, VS Code and
  JetBrains read and write one record.
- **A conversation without a shared topic is a temporary chat.** It stays in the current window, and the status
  line says it is not synced.
- **Only completed messages are stored.** While an answer is still streaming, the other client cannot see it —
  that is not a lost message.
- **Messages are immutable.** The same id with a different payload is an error, not an overwrite.

So for "I sent it here, the other side does not have it": check the status line for "temporary chat" first, then
whether the message finished generating.

---

## Closing the window does not quit

**By design.** The red button only hides the window: an answer being generated must not be cut off by it, and
Warden is the parent of the `repotory-mcp` child — quitting tears that resident channel down.

**To quit**: tray icon → Quit (or `⌘Q`). This is the opposite of Cinder Desktop, which exits when its window
closes because it hosts no long-lived channel.
