# Roadmap

> Source: the "总体路线" discussion draft of 2026-10-28. This page turns it into the executable version that lives in the repository.
> Positioning and boundary are in [product.md](product.md), which also records where the roadmap and Repotory's current state disagree.

## The shape

```
Repotory MCP memory layer     ↑ lightweight Agent Shell (Tauri)      ↑ local models (Ollama / llama.cpp / MLX)
                             ↑ tool layer (Git / Shell / file index / database CLI)
                             ↑ mainstream clients (Claude Code / Cursor / VS Code / JetBrains)
```

The Shell is not closed: the memory layer is open over MCP; the Shell itself only does "local model entry + memory panels + task confirmation".

## Phase 0: repository and positioning (1 week) — current phase

Split the Agent Shell out of Repotory explicitly while keeping it tightly bound.

- Create the `repotory-agent-shell` repository;
- Position it: a local Agent Shell that talks to Repotory MCP by default;
- Lock the stack: Tauri v2 + Rust backend + TS frontend;
- Ollama first as the model entry, llama.cpp / MLX later;
- Default model Qwen3 8B Q4, proven on an M4 Pro first;
- Define the first-version boundary: chat, read project, run commands, memory panels, human confirmation.

**Progress so far**: the repository and skeleton are in place, the positioning/boundary/decisions live in [product.md](product.md), and the first screen shows the detected memory layer and model runtimes.

## Phase 1: wire up the MCP memory layer (1–2 weeks)

Make the Shell consume Repotory's memory reliably.

- Confirm these MCP tools are stable: `repotory_context`, `repotory_handoff`, `repotory_notes_search`, `repotory_projects_list`, `repotory_scan` (the original roadmap's `repotory_pending` and `repotory_snapshot` do not exist in Repotory — see [product.md, open question 1](product.md#1-repotory_pending-and-repotory_snapshot-do-not-exist-in-repotory));
- Implement the MCP client on the Shell side:
  - auto-discover the local Repotory MCP;
  - check the connection at startup;
  - degrade to plain chat when it fails;
- Minimal proof:
  - open a repository;
  - the Shell calls `repotory_context`;
  - the context goes into the model request;
  - the answer is written back with `repotory_handoff`.

No UI polish in this phase — the data path comes first.

## Phase 2: the smallest usable chat shell (1–2 weeks)

- A simple chat interface;
- A Rust backend that proxies the local model API;
- Streaming output;
- Attach the current repository path;
- Attach the recent git diff;
- Attach context retrieved from Repotory;
- Every model request goes through Rust; no port is exposed to the frontend.

Goal: open the Shell, pick a repository, talk to the local model about the project.

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
- **Pending knowledge**: Repotory's pending knowledge, with approve / reject;
- **Cross-session memory**: last session's conclusions, handoff summary, tasks that can continue.

This is the phase that separates the Shell from a plain chat tool. How approve / reject relates to ADR-0015's human gate is [product.md, open question 2](product.md#2-phase-4s-approve--reject-collides-with-adr-0015s-human-gate).

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
- A minimal VS Code / JetBrains adapter;
- A README that says it plainly: the Shell is the entry point, Repotory is the memory, MCP is the open protocol.

## Milestones

| Milestone | Done when |
|---|---|
| M1 | It can call Repotory MCP |
| M2 | It can talk to a local model about the current repository |
| M3 | It can safely run Git / Shell commands |
| M4 | It can see and manage cross-session memory |
| M5 | It installs and initializes in one step |
| M6 | It ships as a standalone product |

## The trade-off

Do not turn the Shell into a "full IDE" on day one. Make it Repotory's best local entry point first: open it and chat about the project, see the memory, run commands safely, continue where the last session stopped. Once that path works, consider Claude Code, Cursor, VS Code and JetBrains — or turning the Shell into a more general local Agent platform.
