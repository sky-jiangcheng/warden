# Repotory Agent

A **copilot** that runs on your own machine: open a repository, talk to a local model about that project, and let [Repotory](https://github.com/sky-jiangcheng/repotory) remember what matters.

> **Status: Phase 2 — model providers are configurable and the chat loop works.** The app builds and runs, it reaches the memory layer, and it talks to any model behind an OpenAI-compatible API.

## What this is

An **independent product**, not one more screen inside the Repotory desktop app.

It does four things: **finds the repositories on this machine by itself**, **talks to a model
about the project you have open**, **runs commands within a whitelist**, and **pulls memory
back from Repotory**. Which model you use is **your**
choice — **any provider that conforms to the OpenAI-compatible API works** (OpenRouter, OpenAI,
DeepSeek, SiliconFlow, vLLM, LM Studio, and the local Ollama too) — and so is the shape of the
interface.

The three roles stay separate:

| | Owner | What it owns |
|---|---|---|
| copilot | Agent (this repo) | Repository discovery, model entry, command execution, memory panels |
| memory | [Repotory](https://github.com/sky-jiangcheng/repotory) | Project knowledge, session handoffs, approved knowledge pages |
| protocol | MCP | The Agent reads memory through `repotory_*` tools only — never the database |

## How it relates to the Repotory desktop app

**Two products, one job each** — not master and servant, not two interfaces of one product.

| | Repotory desktop | Agent (the copilot) |
|---|---|---|
| Positioning | the **workbench** | the **assistant** |
| What it does | browse, edit, **approve**, govern | read, chat, execute |
| Memory channel | uses `internal/service` directly | **MCP only** |
| Approval (approve / reject) | **yes** (a human is present) | **no** — those two tools are not on the MCP surface at all |
| Closing it | the workbench is gone | the copilot keeps running |
| Where the repositories come from | its own scan roots | **its own scan roots + its own watchlist** |

**Approval stays with the human, and not by convention**: Repotory's MCP surface registers only the
pure-read knowledge page tools (`repotory_wiki_search` / `repotory_wiki_read`), because the gate on
the write side is "publishing content into retrieval is a human act". The Agent goes over MCP, so it
stands outside that gate automatically.

**The Agent never touches the database**: it needs the `repotory-mcp` binary to exist.
**Without Repotory installed there is no memory** — the first screen says so plainly rather than
pretending.

## What runs today

- **Find the repositories yourself**: it scans the directories you give it for git repositories,
  lists them, and **a new one surfaces as a bubble at the top of the window asking whether you
  want to follow it**; **a dismissed repository is never asked about again**
- **Configure a model provider**: type a base URL and any OpenAI-compatible service connects;
  **fetch the model list** (whatever the server reports is what is shown — **this repository keeps no
  copy of it**), **test the connection** (a real request, which proves the key, the endpoint and the
  model all work), and pick a model
- **Chat with the model**: the answer streams in as it is written
- **Actually reaches the memory layer**: it launches the `repotory-mcp` child process over stdio by
  itself, does one MCP handshake, then fetches the **real tool list** with `tools/list` and shows it on
  the first screen (**18 tools** on this machine — **this repository keeps no copy of the list**; it
  shows whatever the server reports)
- Shows the first-version product boundary and the milestones

Command execution and the memory panels do not exist yet — see the [roadmap](docs/roadmap.md).

## How the repositories are found

**The copilot walks the disk itself, without going through Repotory.** Give it a few
directories to look in and it finds the git repositories under them, lists them, and
**lets you say which ones are yours**.

- **What counts as a repository**: a `.git` **directory**. A worktree's or a submodule's
  `.git` is a **file**, and that does not count — the same rule Repotory's own scanner
  uses, so the two never disagree about the same directory.
- **Two answers**: "follow" means "this is my work", "dismiss" means "stop asking".
  **A dismissed repository never surfaces again** — otherwise saying "no" once would still
  be asked a second time.
- **A bubble for new repositories**: one at a time, sliding down from the top of the window
  (five cards at once is a form, not a question). **When the window is in the background it
  becomes a system notification, and clicking it brings the window back.**
- **It keeps looking while nobody is looking**: the scan runs every five minutes even with
  the window closed. That is what makes it a robot rather than a report.
- **The watchlist is the copilot's own** (`repos.json` in the app config directory) and
  **does not go into Repotory's database**: the writer of `projects.is_starred` there is the
  desktop app and there is no MCP twin for it — sharing it would mean changing the main
  repository, which is not a copilot's job.

The list carries names and paths for now. **Git metadata (branch, last commit) and "bring a
repository into the conversation" are not done.**

## How model providers are reached

**One protocol covers them all.** The OpenAI-compatible API is the de-facto standard here, and its
shape is small:

```
GET  {base}/models            → {"data": [{"id": "..."}, ...]}
POST {base}/chat/completions  → {"choices": [{"message": {"content": "..."}}]}
POST {base}/chat/completions  (with "stream": true) → SSE lines, ended by data: [DONE]
```

OpenRouter, OpenAI, DeepSeek, SiliconFlow, vLLM, LM Studio and Ollama's `/v1` surface all speak
it, so **there is exactly one client in the code** — adding a vendor is a configuration change,
not a code change.

**A local Ollama takes the same road** (`http://127.0.0.1:11434/v1`): "local model" and "hosted
model" are not two kinds of thing here, they are one shape.

**The API key lives in the operating system's credential store** (macOS Keychain, Windows Credential
Manager, Linux Secret Service); the config file holds only the fact that a key exists — **a key in
a plain file is a key in every backup**.

**Loopback never goes through a proxy**: a local provider is reached directly, everything else goes
through `HTTPS_PROXY` / `HTTP_PROXY` (and `NO_PROXY` is honoured). That is not a preference — with
a proxy running, the proxy is a different machine, and its `127.0.0.1` is itself.

## How the memory layer is reached

MCP over stdio is **newline-delimited JSON-RPC**. The client's handshake is three steps:

```
initialize (a request with an id)
  → notifications/initialized (a notification, no id)
  → tools/list / tools/call
```

The client is written in Rust (`src-tauri/src/mcp_client.rs`) — **a Tauri webview cannot spawn a child
process**, and the IDE plugins can do it in TypeScript only because the VS Code / JetBrains hosts carry
a Node / JVM runtime. The copilot's equivalent host is the Rust side.

The connection is **resident**: the server keeps the same SQLite database open, so re-opening it per
call means re-opening the database per call. The first screen tells three states apart and never blurs
them: **not installed** / **installed but not answering** (with a retry button) / **connected with N
tools**.

## Stack

Tauri v2, Rust backend, TypeScript frontend. The default provider is the local Ollama and the
default model is `qwen3:8b` (Qwen3 8B Q4) — it needs no account and no key, so it works the moment
it is installed.

**Why Tauri**: it ships a **tray** (menu-bar icon, click to wake), which is what makes "close the
workbench and an assistant is still there" possible at all. (Wails v2 has no tray — the `TrayMenu`
in `pkg/menu` is a dead type — and the external `systray` needs cgo.)

**The tray is in place** (`src-tauri/src/tray.rs`): left click wakes the window, right click opens
the Show / Quit menu. **Closing the window still ends the app** — the tray is a wake-up entry for a
process that is already running, not a "hide the window and keep an icon" kind of residency.

**It is "an independent process + a real tray + a real distribution channel", not a native UI
framework** — Tauri uses the system WebView, not Swift/AppKit controls.

## Development

```bash
npm install
npm run tauri dev      # develop
npm run tauri build    # package
```

Type-check the Rust backend on its own:

```bash
cd src-tauri && cargo check
```

## Docs

- [Positioning and boundary](docs/product.md) — what the first version does, does not do, and what is still undecided
- [Roadmap](docs/roadmap.md) — Phase 0–6 and the milestones
- [AGENTS.md](AGENTS.md) — development entry point (for AI agents and new members)

## License

MIT — see [LICENSE](LICENSE).
