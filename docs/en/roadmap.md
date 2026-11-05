# Roadmap

> Source: the "总体路线" discussion draft of 2026-10-28. This page turns it into the executable version that lives in the repository.
> Positioning and boundary are in [product.md](product.md), which also records where the roadmap and Repotory's current state disagree.

## The shape

```
Repotory MCP memory layer     ↑ copilot (Agent, Tauri)             ↑ local models (Ollama / llama.cpp / MLX)
                             ↑ tool layer (Git / Shell / file index / database CLI)
                             ↑ mainstream clients (Claude Code / Cursor / VS Code / JetBrains)
```

The memory layer is open over MCP; the copilot itself only does "local model entry + command execution + memory panels".
Here **"copilot" means this product** (Repotory Agent) — **not** GitHub Copilot.

**It and the Repotory desktop app are two products**: the desktop app is the **workbench** (browse,
edit, approve, govern), the copilot is the **assistant** (read, chat, execute). They share the memory
layer and the protocol — not the interface, and not the lifecycle.

## Phase 0: repository and positioning (1 week) — done

Split the copilot out of Repotory explicitly, making it **its own product**, while keeping it tightly
bound.

- Create the `repotory-agent` repository (formerly `repotory-agent-shell`, renamed on 2026-11-04);
- Position it: an **independent copilot** that talks to Repotory MCP by default;
- Lock the stack: Tauri v2 + Rust backend + TS frontend;
- Ollama first as the model entry, llama.cpp / MLX later;
- Default model Qwen3 8B Q4, proven on an M4 Pro first;
- Define the first-version boundary: chat, read project, run commands, memory panels, human confirmation.

**Progress so far**: done. The repository and skeleton are in place, the positioning/boundary/decisions live in [product.md](product.md), and the first screen shows the detected memory layer, model runtimes, and the **wake-up entry (the tray)**.
The CI gates (docs mirror, frontend build, `cargo fmt` / `clippy` / `test`), the brand icon, and the tray (ADR-0047 decision 5) are all in place.

## Phase 1: wire up the MCP memory layer (1–2 weeks) — the data path is done

Make the copilot consume Repotory's memory reliably.

- Confirm these MCP tools are stable: `repotory_context`, `repotory_handoff`, `repotory_notes_search`, `repotory_projects_list`, `repotory_scan` (the original roadmap's `repotory_pending` and `repotory_snapshot` do not exist in Repotory — see [product.md, open question 1](product.md#1-repotory_pending-and-repotory_snapshot-do-not-exist-in-repotory));
- **Fetch the tool list dynamically**: use MCP's `tools/list` in place of the hardcoded copy (it has
  already drifted — see [product.md, the memory-layer contract](product.md#the-memory-layer-contract));
- Implement the MCP client on the Agent side (**copy the path the two IDE plugins already walk**:
  spawn the `repotory-mcp` child process over stdio):
  - auto-discover the local Repotory MCP;
  - check the connection at startup;
  - degrade to plain chat when it fails;
- Minimal proof:
  - open a repository;
  - the Agent calls `repotory_context`;
  - the context goes into the model request;
  - the answer is written back with `repotory_handoff`.

No UI polish in this phase — the data path comes first.

**Progress so far**: **the data path is done**.

- **A Rust MCP stdio client** (`src-tauri/src/mcp_client.rs`): `initialize` → `notifications/initialized` → `tools/list` / `tools/call`,
  one handshake and then **it stays resident** (the server keeps the same SQLite database open, so re-opening it per call means re-opening the database per call);
- **The tool list is fetched dynamically**: what the first screen shows is what the server just reported (**18 tools** on this machine),
  and this repository **keeps no copy of it** — there is a self-check test in `memory_layer.rs` that watches for exactly that;
- **Three states on the first screen**: not installed / installed but not answering / connected with N tools — none of them is mistaken for another;
- **End-to-end tests**: a real pipe and a fake MCP server, covering the handshake, the tool list, the call round trip, the server's error, and **that the process really is killed**.

**The one loop that is still open**: "put the context into the model request → write the answer back with `repotory_handoff`".
The chat interface landed in Phase 2, so this is now **M2's work**, not "waiting for Phase 2".
The policy for "degrade to plain chat when the connection fails" is **settled** (see "Open questions" item 3
in [product.md](product.md)): **the chat continues and a banner says so out loud** — when the memory layer
cannot be reached the chat still works, but the interface has to say "this turn has no memory", and
**a failed context assembly must never be swallowed**.

## Phase 2: the smallest usable chat shell (1–2 weeks) — done

- A simple chat interface;
- A Rust backend that proxies the model API;
- Streaming output;
- **Model providers are configurable**: anything that conforms to the OpenAI-compatible API works;
- Attach the current repository path;
- Attach the recent git diff;
- Attach context retrieved from Repotory;
- Every model request goes through Rust; no port is exposed to the frontend.

Goal: open the copilot, pick a repository, talk to a model about the project.

**Progress so far**: **the chat loop works and the model entry is open**.

- **The provider configuration layer** (`src-tauri/src/providers.rs`): a provider is "a base URL".
  The configuration lives in a JSON file in the app config directory, and the **API key lives in the
  operating system's credential store** (macOS Keychain / Windows Credential Manager / Linux Secret
  Service) — the config file holds only whether a key exists, because **a key in a plain file is a key
  in every backup**;
- **The OpenAI-compatible client** (`src-tauri/src/llm.rs`): **there is exactly one client in the
  code**. OpenRouter, OpenAI, DeepSeek, SiliconFlow, vLLM, LM Studio and Ollama's `/v1` surface all
  speak the same thing, so adding a vendor is a configuration change rather than a code change;
  **a local Ollama takes the same road** — "local model" and "hosted model" are not two kinds of thing;
- **Fetching the model list**: the list comes from the server, **this repository keeps no copy**;
- **Testing the connection**: a real minimal request, which proves the key, the endpoint and the model
  all work — pinging the port proves none of them;
- **Streaming chat**: `tauri::ipc::Channel` pushes the deltas to the frontend, so the answer appears
  while it is written;
- **The proxy rule**: loopback is reached directly, everything else goes through `HTTPS_PROXY` /
  `HTTP_PROXY` (and `NO_PROXY` is honoured).

**Still not done**: attaching the repository path / git diff / Repotory context, and writing the answer
back with `repotory_handoff`. The notion of "the current repository" landed in Phase 2.5, so these two
are **wiring, not a blocked prerequisite** any more.
The layered policy for what may be written is **settled** (see "Open questions" item 5 in
[product.md](product.md)): `handoff` / `scan` are written automatically, while **durable knowledge
(`notes_create` / `notes_update` / `notes_organize`) needs a human confirmation first**.

## Phase 2.5: finding the repositories itself (1 week) — done

The copilot goes from "one front end of Repotory" to "an independent robot on this machine".

- **It walks the disk itself**: git repositories under the roots the user configured
  (a `.git` *directory* counts; a worktree's `.git` *file* does not), **without going
  through Repotory's scanner and without reading its database**;
- **A list, and the user decides**: the repositories are listed, and the user says which
  ones are theirs;
- **Follow or dismiss**: two answers. "Follow" means "this is my work"; "dismiss" means
  "stop asking". **A dismissed repository never surfaces again** — otherwise saying "no"
  once would still be asked a second time;
- **A bubble for new repositories**: one repository at a time (five cards at once is a
  form, not a question). The bubble slides down from the top of the window; when the
  window is in the background it becomes a system notification, and **clicking the
  notification brings the window back**;
- **It keeps looking while nobody is looking**: the scan runs every five minutes even
  with the window closed. That is what makes it a robot rather than a report.

**The watchlist is the copilot's own** (`repos.json` in the app config directory) and
**does not go into Repotory's database**: the writer of `projects.is_starred` there is the
desktop app (desktop-only), and there is no MCP twin for it — **sharing it would mean
changing the main repository, which is not a copilot's job**.

**Not done**: feeding a repository into a model request (which repository the
conversation is about), and git metadata (branch, last commit). The list carries names
and paths for now. **"The current repository" is in place — what is left is wiring, not a new concept.**

## Phase 3: the tool execution layer (2–3 weeks)

Whitelisted tools first:

`git status` · `git diff` · `git log` · `git branch` · `git show` · `ls / tree` · `cat / head / grep` · `npm run build` · `cargo test` · `mysql -e` (read-only first)

Execution rules:

- every write needs confirmation;
- every command is shown in full;
- every output can be collapsed;
- dangerous commands are blocked by default;
- every run is recorded in a local log.

No fully automatic Agent in this phase — human-in-the-loop first.

## Phase 4: the memory panels (1–2 weeks)

Three panels:

- **Current context**: the current repository, branch, recent diff, injected memory;
- **Approved knowledge**: a read-only view over `repotory_wiki_search` (**pages awaiting approval are
  invisible to the copilot — that is the workbench's business**, see
  [product.md, open question 2](product.md#2-phase-4s-approve--reject-collides-with-adr-0015s-human-gate-resolved-option-c));
- **Cross-session memory**: last session's conclusions, handoff summary, tasks that can continue.

This is the phase that separates the copilot from a plain chat tool. **The panels are read-only** —
approving always happens in the Repotory desktop app.

## Phase 5: local model management (1–2 weeks)

- Detect whether Ollama is installed;
- Installation guidance when it is not;
- Pull the default model automatically;
- Switch models;
- Show model state, latency, VRAM/RAM usage;
- Offline-mode hints.

llama.cpp / MLX arrive as pluggable backends, never hard-coded. The first screen already shaped the backend registry that way (see `src-tauri/src/model_backends.rs`).

## Phase 6: distribution and shipping (1–2 weeks)

- macOS dmg packaging;
- Optional Homebrew cask;
- The App Store build ships no model and points at Ollama;
- A `repotory-agent init` one-shot initializer;
- A README that says it plainly: the copilot is the entry point, Repotory is the memory, MCP is the
  open protocol;
- **The version coupling written down**: which copilot version pairs with which Repotory version
  (without a capability handshake that is an implicit constraint — see
  [product.md, open question 4](product.md#4-version-coupling-with-repotory)).

## Milestones

| Milestone | Done when | State |
|---|---|---|
| M1 | It can call Repotory MCP | done (Phase 1) |
| M1.5 | **It can find the repositories itself and let the user say which ones matter** | **done (Phase 2.5)** |
| M2 | It can talk to a model about the current repository | in progress (Phase 2: the chat loop works, the repository context is not wired) |
| M3 | It can safely run Git / Shell commands | not started |
| M4 | It can see and manage cross-session memory | not started |
| M5 | It installs and initializes in one step | not started |
| M6 | It ships as a standalone product | not started |

## The trade-off

Do not turn the copilot into a "full IDE" on day one. Make it a good local assistant first: open it and chat about the project, see the memory, run commands safely, continue where the last session stopped. Once that path works, consider Claude Code, Cursor, VS Code and JetBrains — or turning it into a more general local Agent platform.

**The tray is in place** (ADR-0047 decision 5): it is the carrier of "close the workbench and
an assistant is still there". The tray icon is the copilot's wake-up entry — click it to bring the
window back, right-click for the menu (Show / Quit).
**Closing the window still ends the app**: the tray is a wake-up entry for a process that is
*already running*, not a "hide the window and leave only an icon" kind of residency — that trades
one unreachable window for one unreachable process.
**The exact wake-up interaction is still decided by user feedback** (a global shortcut and an
always-visible window are both not built yet).
