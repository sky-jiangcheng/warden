# Positioning and boundary

> This page settles three things for Phase 0: what the Shell is, how far the first version goes, and which decisions are left to the maintainer.
> The phases and milestones live in [roadmap.md](roadmap.md).

## One line

**Repotory Agent Shell is the entry point to a local Agent, not another Agent platform.** It turns "talk to a local model about the project in front of you" into a window, and leaves "what to remember" to Repotory.

## Three roles, three hard boundaries

| Role | Owner | Owns | Does not own |
|---|---|---|---|
| Entry point | Agent Shell (this repo) | Local model entry, memory panels, task confirmation | Retrieval ranking, project knowledge storage |
| Memory | [Repotory](https://github.com/sky-jiangcheng/repotory) | Project context, session handoffs, pending knowledge | Deciding what an Agent looks like |
| Protocol | MCP | The only channel between the Shell and the memory layer | —— |

The three boundaries are hard:

1. **The Shell does not re-implement Repotory's retrieval.** Context comes from `repotory_context`; it does not rank FTS itself, does not compute embeddings itself, and keeps no vector store of its own.
2. **The Shell does not touch Repotory's database.** MCP tools only. The database is Repotory's private implementation, and the Shell has no business knowing it exists.
3. **The Shell is not a full IDE.** Routing belongs to Repotory (`repotory_notes_read` / `repotory_notes_update` and friends); the Shell only consumes it.

## The first-version boundary

Five capabilities, fixed by Phase 0. Together they are version one:

| Capability | Means | Done when |
|---|---|---|
| Chat | Talk to the local model about the current project, streamed | Pick a repository + ask a question → get an answer |
| Read project | Attach the repository path, the recent git diff, context retrieved from Repotory | Every attachment is selectable and previewable |
| Run commands | Whitelisted tool execution; writes need human confirmation | The full command is shown, dangerous ones are blocked by default, every run is logged |
| Memory panels | Current context / pending knowledge / cross-session conclusions | All three panels show real data |
| Human confirmation | Dangerous actions never run unattended | The user sees the full command and its blast radius before confirming |

## Decisions fixed in Phase 0

| Decision | Content | Cost |
|---|---|---|
| Stack | Tauri v2, Rust backend, TypeScript frontend | Not the same stack as the Repotory desktop app (Wails/Go). The two share data and protocol, nothing else |
| Model entry | Ollama first; llama.cpp / MLX arrive as pluggable backends, never hard-coded | The backend registry has to exist up front (`src-tauri/src/model_backends.rs`) |
| Default model | `qwen3:8b` (Qwen3 8B Q4), proven on an M4 Pro first | Switching models means the user pulls it in Ollama |
| Memory channel | MCP, never a private store | The Shell depends on Repotory's MCP binary being present |

The model backend registry is already shaped for "pluggable": Ollama is the only implementation, `llama.cpp` and `MLX` are declared `planned`, and the UI asks one command for the whole list — adding a backend does not touch the frontend.

## The memory-layer contract

Every MCP tool the Shell consumes comes from Repotory's existing `repotory_*` tool set (the Repotory repository is the source of truth):

- `repotory_context` — project context; Phase 1's main data source
- `repotory_handoff` — writes the session handoff; Phase 1's exit
- `repotory_notes_search` / `repotory_notes_read` / `repotory_notes_create` / `repotory_notes_list` / `repotory_notes_update` — note reads and writes
- `repotory_projects_list` / `repotory_projects_stats` — the project list
- `repotory_scan` — discovering local Git repositories

A copy of the list lives in `src-tauri/src/memory_layer.rs` for the first screen only. It is **not** the contract.

## Open questions (the maintainer decides)

### 1. `repotory_pending` and `repotory_snapshot` do not exist in Repotory

Two of the four tools the original roadmap listed for Phase 1 are not there: `repotory_context` and `repotory_handoff` are real, but `repotory_pending` and `repotory_snapshot` have no counterpart in Repotory's MCP tool set.

- "Pending knowledge" in Repotory is really the wiki page status `pending / approved / rejected` (`internal/db/wiki.go`);
- A snapshot is Repotory's local backup mechanism (ADR-0025 L1) — an internal capability, not an MCP tool.

**Open**: which interface the Shell's "pending knowledge" panel reads.

### 2. Phase 4's approve / reject collides with ADR-0015's human gate

Repotory marks `ListPendingWikiPages` / `ApproveWikiPage` / `RejectWikiPage` as desktop-only (ADR-0015 decision 3: publishing content into retrieval is a human act and has no other caller). A Shell memory panel that can approve or reject has to cross that gate:

- **Option A**: the Shell talks to the local RPC bridge (the same path the JetBrains plugin uses) instead of MCP;
- **Option B**: Repotory grows a controlled MCP tool that only allows the write while a human is present;
- **Option C**: Phase 4 shows a read-only panel, and approving still happens in the Repotory desktop app.

**Open**: which one. It decides the size of Phase 4, and whether the Shell must depend on a local service rather than only on an MCP process.

### 3. How automatic is "auto-discovering" the MCP client?

Phase 1 of the roadmap says "auto-discover the local Repotory MCP". The options are: find the `repotory-mcp` binary on PATH, a fixed path, or a user-configured one. The first screen probes PATH plus the common install directories (`/usr/local/bin`, `/opt/homebrew/bin`, `~/.local/bin`) and only reports **presence** — it never launches anything.

**Open**: whether the Shell launches the MCP process at startup, and how a failed connection degrades (the roadmap says "fall back to plain chat").

### 4. Version coupling with Repotory

The MCP tool set the Shell consumes evolves with Repotory (`repotory_*` tools are added and removed). Does the Shell need a capability handshake ("which tools do you have?") instead of assuming a list, or is a semantic-version constraint on Repotory enough?

**Open**: without a handshake, the Shell↔Repotory version combination is an implicit constraint that has to be written down in the README and the installer.
