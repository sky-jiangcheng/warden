# Repotory Agent

A **copilot** that runs on your own machine: open a repository, talk to a local model about that project, and let [Repotory](https://github.com/sky-jiangcheng/repotory) remember what matters.

> **Status: Phase 1 — the MCP memory layer is wired up and the data path works.** The app builds and runs, it reaches the memory layer; the chat loop lands in Phase 2.

## What this is

An **independent product**, not one more screen inside the Repotory desktop app.

It does three things: **talks to a local model about the project you have open**, **runs commands
within a whitelist**, and **pulls memory back from Repotory**. Which model backend you use is
**your** choice (Ollama first, llama.cpp / MLX as pluggable backends), and so is the shape of the
interface.

The three roles stay separate:

| | Owner | What it owns |
|---|---|---|
| copilot | Agent (this repo) | Local model entry, command execution, memory panels |
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

**Approval stays with the human, and not by convention**: Repotory's MCP surface registers only the
pure-read knowledge page tools (`repotory_wiki_search` / `repotory_wiki_read`), because the gate on
the write side is "publishing content into retrieval is a human act". The Agent goes over MCP, so it
stands outside that gate automatically.

**The Agent never touches the database**: it needs the `repotory-mcp` binary to exist.
**Without Repotory installed there is no memory** — the first screen says so plainly rather than
pretending.

## What runs today

- Detects the local model runtimes (Ollama first; llama.cpp / MLX reserved as pluggable backends)
- **Actually reaches the memory layer**: it launches the `repotory-mcp` child process over stdio by
  itself, does one MCP handshake, then fetches the **real tool list** with `tools/list` and shows it on
  the first screen (**18 tools** on this machine — **this repository keeps no copy of the list**; it
  shows whatever the server reports)
- Shows the first-version product boundary and the milestones

Chat, command execution and the memory panels do not exist yet — see the [roadmap](docs/roadmap.md).

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

Tauri v2, Rust backend, TypeScript frontend. Ollama is the first model entry point; the default
model is `qwen3:8b` (Qwen3 8B Q4).

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
