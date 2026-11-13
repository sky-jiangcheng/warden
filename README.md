<p align="center">
  <a href="https://github.com/sky-jiangcheng/warden"><img src="src-tauri/icons/128x128.png" width="96" alt="Warden" /></a>
</p>

<h1 align="center">Warden</h1>

<p align="center">
  <b>The copilot that is still there when the editor is closed.</b><br/>
  A local assistant in your menu bar. It finds the repositories on your machine, keeps a small<br/>
  companion window on screen, and talks about the project you have open — over MCP, to any model you point it at.
</p>

<p align="center">
  <b>Two products, one job each.</b><br/>
  <a href="https://github.com/sky-jiangcheng/cinder">Cinder</a> Desktop is the <b>workbench</b> — browse, edit, <b>approve</b>, govern what is remembered.<br/>
  Warden is the <b>assistant</b> — read, chat, act. One shared local memory, one protocol,<br/>
  and <b>approval stays in Cinder Desktop</b>, because the approval tools are not on the MCP surface Warden is allowed to use.
</p>

<p align="center">
  <a href="https://github.com/sky-jiangcheng/warden/releases"><img src="https://img.shields.io/github/v/release/sky-jiangcheng/warden?color=6366f1" alt="Release" /></a>
  <a href="https://github.com/sky-jiangcheng/warden/actions/workflows/ci.yml"><img src="https://github.com/sky-jiangcheng/warden/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <img src="https://img.shields.io/badge/Platforms-macOS%20%7C%20Windows%20%7C%20Linux-111827" alt="macOS | Windows | Linux" />
  <img src="https://img.shields.io/badge/Account-not%20required-22c55e" alt="No account required" />
  <img src="https://img.shields.io/badge/Model-any%20OpenAI--compatible-7c3aed" alt="Any OpenAI-compatible model" />
  <a href="./LICENSE"><img src="https://img.shields.io/github/license/sky-jiangcheng/warden?color=22c55e" alt="MIT License" /></a>
</p>

<p align="center">
  English | <a href="README.zh-CN.md">简体中文</a> · <a href="docs/en/product.md">Positioning</a> · <a href="docs/en/roadmap.md">Roadmap</a> · <a href="docs/en/troubleshooting.md">Troubleshooting</a>
</p>

---

## 🎯 Why a second app at all?

Because the moment you close the editor, every coding agent forgets the project — and the assistant you
wanted was never only for one window.

| The problem today | What Warden does instead |
| :--- | :--- |
| 🪟 **The assistant lives inside the editor.** Close the window and it is gone, along with the thread. | **A resident process with a tray.** Close the window and it hides; quit from the tray and it exits. A question you started keeps generating while the window is in the background. |
| 🌙 **You lose the "where was I".** Yesterday's decisions, the gotcha you hit, the next step — none of it survives the session. | **It reads the same memory your agents write.** Project knowledge, session handoffs and approved knowledge pages come from Cinder over MCP — the same records Cursor wrote, not a second history that looks like it. |
| 🧠 **Long answers need a big model; quick questions do not.** | **Any OpenAI-compatible provider.** One client, no vendor code: a local Ollama for free and offline, OpenRouter / OpenAI / DeepSeek when the question is hard. Switching is a base URL. |
| 📂 **You tell every tool where your code is.** | **It walks the disk itself.** Point it at directories, it finds the git repositories under them, and a new one surfaces as a bubble asking whether it is yours. Dismissed once, never asked again. |

---

## 📦 The two products

Not master and servant, and not two windows of one application — two halves of one loop.

| | Cinder Desktop | Warden |
| :--- | :--- | :--- |
| Positioning | the **workbench** | the **assistant** |
| What it does | browse, edit, **approve**, govern | read, chat, execute |
| Memory channel | uses the service layer directly | **MCP only** — never the database |
| Approval (approve / reject) | **yes** — a human is present | **no** — those tools are not on Warden's MCP surface at all |
| If you close it | the workbench is gone | the copilot keeps running |
| Where repositories come from | its own scan roots | **its own scan roots + its own watchlist** |

**Approval stays with the human by construction, not by convention.** Cinder's MCP surface registers only
the read-only knowledge-page tools; the gate on the write side is "publishing content into retrieval is an
act of a person". Warden reaches memory over MCP, so it stands outside that gate automatically — there is
no flag to get wrong.

**No `repotory-mcp`, no memory.** Warden launches the MCP server as a child process. That binary ships as a
standalone release asset for every platform, so **Cinder Desktop is not a prerequisite** — the first screen
says which of the three states it is in rather than pretending. (Cinder 2.33 renamed its binaries; the same
server is also shipped as `cinder-mcp`. Warden looks for the frozen name `repotory-mcp`, which Cinder still
installs alongside it.)

---

## 🚀 Install

### macOS

Download `Warden_<version>_universal.dmg` from [Releases](https://github.com/sky-jiangcheng/warden/releases)
(one universal build covers Apple silicon and Intel) and drag it to `/Applications`. The dmg is signed with an
Apple Developer ID and notarised, so Gatekeeper lets it through — double-click and it opens, no dialog.

Releases before v0.1.8 were not signed. On one of those, macOS refuses to open the app with *"Apple cannot
check Warden for malicious software"*; [here is what to do about that](docs/en/troubleshooting.md#macos-refuses-to-open-the-app).

### Windows · Linux

| Platform | Asset | Note |
| :--- | :--- | :--- |
| Windows | `Warden_<version>_x64_en-US.msi` / `_x64-setup.exe` | unsigned — SmartScreen may ask you to confirm |
| Linux | `Warden_<version>_amd64.AppImage` / `Warden_<version>_amd64.deb` / `Warden-<version>-1.x86_64.rpm` | AppImage is the no-install option |

### Before the memory layer works

The chat works with any provider the moment Warden is installed. The **memory** panels need the
`repotory-mcp` binary on your `PATH` — it comes from the
[Cinder](https://github.com/sky-jiangcheng/cinder) releases, or from Cinder Desktop's installer. Without it
the first screen says *not installed* and the rest of the app still works.

### Run `init` once after install

To confirm a machine is ready without opening the window, from a terminal:

```bash
repotory-agent init
```

It checks **config, memory layer, local model** one by one and marks each — `✓` fine,
`!` worth attention (does not block), `✗` needs fixing. The exit code is non-zero only
when something is `✗` (so scripts can decide):

```text
Warden initialization check
────────────────────────────────────────────────────────
✓ config: 1 provider, current Ollama (local)
✓ memory layer: connected, 20 tools, memory layer v1.1.0 (/usr/local/bin/repotory-mcp)
! local model: Ollama is installed but http://127.0.0.1:11434/v1 did not answer. Run `ollama serve`
```

A `!` does not block: a stopped local model does not stop you from connecting a
cloud provider. `init` **does not decide for you** — it writes the default config a
first run would write anyway and reports the state; choosing a model is still yours.

### Version coupling: read "memory layer vX.Y.Z" on the first screen, not the build number

Warden and Cinder have no hard version pin — the interface is MCP, and the **contract version**
shown on the first screen comes from the memory layer's own `initialize` answer. **That X.Y.Z
answers "did the tool surface change", not "is the build new"**: release numbers climb daily and
UI changes happen daily, but the tool surface changes a few times a year — a 2.30.0 tells you
nothing about whether the tools you depend on moved. So the first screen says "memory layer
vX.Y.Z", and only a major bump means the surface may have changed. Out of range gets a plain
sentence, never a hard block.

---

## ⚡ Quick start

1. **Open Warden.** No account, no sign-in, no telemetry. It starts in the tray.
2. **Pick a model.** The default is a local Ollama (`http://127.0.0.1:11434/v1`, model `qwen3:8b`) — it needs
   no key, so if Ollama is running there is nothing to configure. Any other provider: paste a base URL, press
   **Test connection** (a real request — it proves endpoint, key and model together), pick a model, done.
3. **Give it a directory to look in.** It lists the git repositories underneath and asks, one at a time,
   whether a newly found one is yours.
4. **Ask.** Click the companion, or press `⌥Space`. The answer streams in as it is written.

---

## ✅ What runs today

- **Finds the repositories itself** — scans the directories you give it every five minutes, even with the
  window closed; a new repository surfaces as a bubble in the window (no system notification),
  and **a dismissed one never surfaces again**
- **Configures any OpenAI-compatible provider** — base URL, model list fetched from the server (Warden keeps
  no copy of anyone's catalogue), a real connection test, optional extra headers
- **Chats, streaming** — and the thread is the **same conversation the IDE plugins see**: topics and messages
  poll `repotory_chat`, so Warden, VS Code and JetBrains read and write one record. Only completed messages
  sync, messages are immutable (same id with a different payload is an error, never an overwrite), and every
  assistant message is labelled from its own `source` — calling a plugin's reply "Warden" would be claiming
  another client's answer
- **A resident companion window** — 132×128, nothing in it but the cat and a one-line bubble; click it or
  press `⌥Space` to open the chat, and only the cat drags the window. The cat follows the pointer with
  its eyes anywhere on screen (the head turn fades with distance), eased per-frame, blinks on its own,
  and sulks when left alone; head and body frames overlap in fur, so a click hops the whole cat without
  ever opening a seam at the neck. **Closing a window hides it**
  (the red button must not cut off an answer being generated); **Quit in the tray is the way out**
- **Numbers on demand** — ask in the chat and the model calls `repotory_metrics`, which reads what memory
  **already stores**: nothing scans the disk, runs git or refreshes a cache to answer you. A scope it cannot
  cover shows `null`, never a guessed number
- **Really reaches the memory layer** — launches the MCP server over stdio, does one handshake, then fetches
  the **real tool list** with `tools/list` and shows it (the server reports; Warden keeps no copy)

**Not built yet:** command execution in the repository, the memory panels, git metadata (branch, last commit)
on the repository list. See the [roadmap](docs/en/roadmap.md) — it is honest about the order things arrive in.

---

## 🧭 How it works

### The three lanes

| Lane | Owner | Reached by | Notes |
| :--- | :--- | :--- | :--- |
| Repositories | Warden | its own scanner (`repos.json` in the app config directory) | a `.git` **directory** is a repository; a worktree's or submodule's `.git` **file** is not — the same rule Cinder uses, so the two never disagree about one directory |
| Providers | Warden | HTTPS to the base URL you configured, loopback directly | **a local provider never goes through a proxy**: with a proxy running, the proxy is another machine and its `127.0.0.1` is itself |
| Memory | [Cinder](https://github.com/sky-jiangcheng/cinder) | MCP over stdio, `repotory_*` tools only | the connection is **resident**: the server holds one SQLite handle, so reconnecting per call would reopen the database per call |

### One protocol for every model

```
GET  {base}/models            → {"data": [{"id": "..."}, ...]}
POST {base}/chat/completions  → {"choices": [{"message": {"content": "..."}}]}
POST {base}/chat/completions  (with "stream": true) → SSE lines, ended by data: [DONE]
```

OpenRouter, OpenAI, DeepSeek, SiliconFlow, vLLM, LM Studio and Ollama's `/v1` surface all speak it, so **there
is exactly one client in the code** — adding a vendor is a configuration change, not a code change. "Local
model" and "hosted model" are not two kinds of thing here; they are one shape.

### Why Tauri

Warden needs a **tray** — a menu-bar icon, click to wake — because "close the workbench and an assistant is
still there" is the entire product. Wails v2 has no tray (`pkg/menu`'s `TrayMenu` is a dead type) and the
standalone `systray` crates need cgo, which would break the bare `go build ./...` in Cinder's CI. Tauri also
gives the app a real frontend toolchain for the companion window and a native spinner-free WebView.

It is **an independent process + a real tray + a real distribution channel**, not a native UI framework:
Tauri uses the system WebView, so the interface is web technology, not Swift/AppKit controls.

---

## ❓FAQ

**Do I need Cinder Desktop installed?**
No. You need the `repotory-mcp` binary on `PATH` for the memory lane. Everything else — repositories, providers,
chat — works without it, and the app says so on the first screen instead of failing at the first question.

**Does my code or my questions leave the machine?**
Only to the provider **you** configured. There is no Warden server, no account, no telemetry, no crash
reporting. With the default Ollama provider, nothing leaves the machine at all.

**Where is my API key stored?**
In a `secrets.json` file beside the provider list, with `0600` permissions — not in the macOS Keychain. That
is a deliberate trade-off and it is explained, with the alternative, in
[why the key is a file, not a keychain item](docs/en/troubleshooting.md#where-the-api-key-lives-and-why).

**macOS says it cannot verify the app. Is that expected?**
Only on releases before v0.1.8 — those were ad-hoc signed. v0.1.8 and later are signed and notarised. See
[macOS refuses to open the app](docs/en/troubleshooting.md#macos-refuses-to-open-the-app).

**Why not just an IDE plugin?**
Because a plugin dies with its host, and because the interesting moment is the one where no editor is open —
you are reading a stack trace in a terminal, or you want to ask "what did I decide about the retry policy"
before opening anything. Warden is the piece that is still running; the IDE plugins are the ones that come
and go. They share the memory, not the process.

---

## 🛠 Development

```bash
npm install
npm run tauri dev      # develop (Vite + Tauri in one command)
npm run tauri build    # package a bundle for this machine
```

Backend on its own, without a window server:

```bash
cd src-tauri && cargo check --all-targets --features test-fixtures
cd src-tauri && cargo test --features test-fixtures     # the suite, including the end-to-end MCP run
```

Gates that CI enforces and you can run locally:

```bash
node scripts/check-brand.mjs        # display-layer brand words, frozen identifiers, live repo addresses
node scripts/check-docs-mirror.mjs  # docs/*.md and docs/en/*.md are 1:1
cargo fmt --all -- --check
cargo clippy --all-targets --features test-fixtures -- -D warnings
```

---

## 📚 Docs

- [Positioning and boundary](docs/en/product.md) — what the first version does, does not do, and what is still undecided
- [Roadmap](docs/en/roadmap.md) — Phase 0–6 and the milestones
- [Troubleshooting](docs/en/troubleshooting.md) — the Gatekeeper dialog, the key file, MCP not found, provider errors
- [AGENTS.md](AGENTS.md) — development entry point (for AI agents and new members)

---

## 📄 License

[MIT](LICENSE) © 2026-present [sky-jiangcheng](https://github.com/sky-jiangcheng) and Warden contributors.

Warden is free for personal and commercial use under the MIT License. In practice that means:

| You want to | MIT allows it |
| :--- | :--- |
| Use Warden at work, on a commercial codebase | ✅ yes |
| Fork it, modify it, keep your changes private | ✅ yes |
| Ship it inside a product you sell | ✅ yes |
| Redistribute a patched build to your team | ✅ yes |

The only thing the license does not hand over is the **name**. The `Warden` and `Cinder` names and logos
remain trademarks of the project, so a fork must not present itself as Warden — rename it, keep the notice,
and link back here. That is the whole restriction.

**Third-party components.** The application bundles [Tauri](https://tauri.app) (MIT / Apache-2.0) and the Rust
crates listed in `src-tauri/Cargo.lock` (mostly MIT / Apache-2.0); their licenses are those of their authors.
No model weights are bundled: Warden ships no model, so the license of the model you connect is between you
and its provider. The `repotory-mcp` binary it launches is part of
[Cinder](https://github.com/sky-jiangcheng/cinder) and carries the same MIT license.

**No warranty.** As the MIT text says: the software is provided "as is". It runs read-only over your
repositories and writes only inside its own configuration directory, but it is pre-1.0 software — pin a
version you have tried, and read the release notes before upgrading.
