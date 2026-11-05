# Positioning and boundary

> This page settles three things for Phase 0: what this copilot is, how far the first version goes, and which decisions are left to the maintainer.
> The phases and milestones live in [roadmap.md](roadmap.md).

## One line

**Repotory Agent is an independent product: a copilot that runs on your own machine — not another Agent platform, and not one more screen inside the Repotory desktop app.** It turns "talk to a model about the project in front of you" into a window, and leaves "what to remember" to Repotory.

## Three roles, three hard boundaries

| Role | Owner | Owns | Does not own |
|---|---|---|---|
| copilot | Agent (this repo) | Model entry, command execution, memory panels | Retrieval ranking, project knowledge storage, **approval** |
| memory | [Repotory](https://github.com/sky-jiangcheng/repotory) | Project context, session handoffs, approved knowledge pages | Deciding what the copilot looks like |
| protocol | MCP | The only channel between the Agent and the memory layer | —— |

The three boundaries are hard:

1. **The Agent does not re-implement Repotory's retrieval.** Context comes from `repotory_context`; it does not rank FTS itself, does not compute embeddings itself, and keeps no vector store of its own.
2. **The Agent does not touch Repotory's database.** MCP tools only. The database is Repotory's private implementation, and the Agent has no business knowing it exists.
3. **The Agent is not a full IDE.** Routing belongs to Repotory (`repotory_notes_read` / `repotory_notes_update` and friends); the Agent only consumes it.

## How it relates to the Repotory desktop app: two products, one job each

**Not master and servant, not two interfaces of one product.**

| | Repotory desktop | Agent (the copilot) |
|---|---|---|
| Positioning | the **workbench** | the **assistant** |
| What it does | browse, edit, **approve**, govern | read, chat, execute |
| Memory channel | uses `internal/service` directly | **MCP only** |
| Approval (approve / reject) | **yes** (a human is present) | **no** — those two tools are not on the MCP surface at all |
| Closing it | the workbench is gone | the copilot keeps running |
| Interface | the three-column shell + the small bot float | its own window + a tray |

**Approval stays with the human, and not by convention.** Repotory's MCP surface registers only two
**pure-read** knowledge page tools (`repotory_wiki_search` / `repotory_wiki_read`), because the gate on
the write side is "publishing content into retrieval is a human act" (ADR-0015 decision 3).
**The Agent goes over MCP, so it stands outside that gate automatically — it cannot cross it even if it
wanted to.**

**The Agent needs the `repotory-mcp` binary to exist before it has any memory.**
**Without Repotory installed there is no memory** — the first screen has to say so plainly rather than
pretending.

## The first-version boundary

Five capabilities, fixed by Phase 0. Together they are version one:

| Capability | Means | Done when |
|---|---|---|
| Chat | Talk to the model about the current project, streamed | Pick a repository + ask a question → get an answer |
| Read project | Attach the repository path, the recent git diff, context retrieved from Repotory | Every attachment is selectable and previewable |
| Run commands | Whitelisted tool execution; writes need human confirmation | The full command is shown, dangerous ones are blocked by default, every run is logged |
| Memory panels | Current context / approved knowledge / cross-session conclusions | All three panels show real data |
| Human confirmation | Dangerous actions never run unattended | The user sees the full command and its blast radius before confirming |

## Decisions fixed in Phase 0

| Decision | Content | Cost |
|---|---|---|
| Stack | Tauri v2, Rust backend, TypeScript frontend | Not the same stack as the Repotory desktop app (Wails/Go). The two share data and protocol, nothing else |
| **Why Tauri** | **it ships a tray** (menu-bar icon, click to wake) — Wails v2 has no tray (the `TrayMenu` in `pkg/menu` is a dead type, and the external `systray` needs cgo). **The tray is in place** (`src-tauri/src/tray.rs`): left click wakes the window, right click opens the Show / Quit menu | **Tauri uses the system WebView, not Swift/AppKit native controls** — it is "an independent process + a real tray + a real distribution channel", **not "a native UI framework"** |
| Model entry | **Any provider that conforms to the OpenAI-compatible API**, no vendor hard-coded | One client covers them all; local and hosted take the same road |
| Default provider | the local Ollama (`http://127.0.0.1:11434/v1`) — no account, no key | Another model means configuring another provider |
| Default model | `qwen3:8b` (Qwen3 8B Q4), proven on an M4 Pro first | Switching models means the user pulls it in Ollama |
| Memory channel | MCP, never a private store | The Agent depends on Repotory's MCP binary being present |

**The model entry is open in Phase 2**: a provider is "a base URL". OpenRouter, OpenAI, DeepSeek,
SiliconFlow, vLLM, LM Studio and Ollama's `/v1` surface all speak the same protocol, so **there is
exactly one client in the code** (`src-tauri/src/llm.rs`) and **adding a vendor is a configuration
change, not a code change**.

The old "local runtime registry" (`model_backends.rs`, with Ollama implemented and `llama.cpp` / `MLX`
declared `planned`) is **deleted**: it treated "local model" and "hosted model" as two kinds of thing,
so every new vendor meant a code change. **The local Ollama now takes the same road as a remote
provider** (`http://127.0.0.1:11434/v1`) — "Ollama is installed" and "OpenRouter is configured" are
the same kind of fact.

**The API key lives in the operating system's credential store** (macOS Keychain, Windows Credential
Manager, Linux Secret Service); the provider config file holds only whether a key exists.

**Everything the user is allowed to have a preference about** — which model, the shape of the
interface — is left to the user.

**The wake-up entry is in place, but the overall shape of the wake-up interaction is still undecided**: the tray icon is the current entry (left click wakes the window, right click opens the menu), and the first screen carries a "wake-up entry" card that shows in real time whether the tray is there — macOS does not let an external process query somebody else's menu-bar status item, so "is the tray actually there" can only be answered by the app itself (`tray.installed`), and when the tray fails to install the log carries a `[tray] ...` line.
**A global shortcut and an always-visible window are not built yet** — they wait for user feedback.

## The memory-layer contract

Every MCP tool the Agent consumes comes from Repotory's existing `repotory_*` tool set (the Repotory repository is the source of truth):

- `repotory_context` — project context; Phase 1's main data source
- `repotory_handoff` — writes the session handoff; Phase 1's exit
- `repotory_notes_search` / `repotory_notes_read` / `repotory_notes_create` / `repotory_notes_list` / `repotory_notes_update` / `repotory_notes_organize` — note reads and writes
- `repotory_projects_list` / `repotory_projects_stats` — the project list
- `repotory_wiki_search` / `repotory_wiki_read` — **approved** knowledge pages (pure reads)
- `repotory_scan` — discovering local Git repositories
- `repotory_ask` — evidence-backed answers with sources (ADR-0033)
- `repotory_agent_score` / `repotory_integrity` — introspection

The tool names' single source of truth is the Repotory main repository, and **this repository keeps no
runtime copy of that list**. Phase 1 deleted that copy: the tool list is fetched dynamically via MCP's
`tools/list` at connection time (`src-tauri/src/mcp_client.rs`), and what the first screen shows is what
the server just reported.
**Phase 0's copy listed 7 names while the server registers 18** — and the missing ones included
`repotory_wiki_search` / `repotory_wiki_read` (the only way to read approved knowledge) and
`repotory_ask` (evidence-backed answers).
**A list that drifts is worse than no list, because it makes people think it is real** — there is now a
self-check test in `memory_layer.rs` that watches for tool names being written back into the source.

> The list above is **for readers, not a runtime contract**: on this machine the server reports **18**
> tools, matching the names listed above. **At runtime the server's answer always wins** — when
> Repotory adds a tool, the copilot sees it without a line of code changing here.

## Open questions (the maintainer decides)

### 1. ~~`repotory_pending` and `repotory_snapshot` do not exist in Repotory~~ (clarified)

Two of the four tools the original roadmap listed for Phase 1 are not there: `repotory_context` and `repotory_handoff` are real, but `repotory_pending` and `repotory_snapshot` have no counterpart in Repotory's MCP tool set.

- "Pending knowledge" in Repotory is really the wiki page status `pending / approved / rejected` (`internal/db/wiki.go`);
- A snapshot is Repotory's local backup mechanism (ADR-0025 L1) — an internal capability, not an MCP tool.

**Conclusion**: **the "pending knowledge" panel = a read-only view over `repotory_wiki_search`.**
There is no "list the pages awaiting approval" tool on the MCP surface — **because "who gets to see
unapproved content" is itself part of the gate**. The panel therefore only shows approved pages, and
**pages awaiting approval are invisible to the copilot** (which is correct: that is the workbench's
business).

### 2. ~~Phase 4's approve / reject collides with ADR-0015's human gate~~ (resolved: option C)

The original question: Phase 4's memory panel wanted approve / reject, but ADR-0015 marks
`ApproveWikiPage` / `RejectWikiPage` as desktop-only ("publishing content into retrieval is a human act
and has no other caller").

**There is nothing to decide any more — the MCP surface only ever had the pure-read knowledge page
tools** (`cmd/mcp/tools_wiki.go` registers `wiki_search` / `wiki_read` only, with a comment putting the
gate on the write side). **Going over MCP puts you outside that gate automatically**, with no extra
mechanism required.

**Conclusion: option C, and it is not a compromise — it is the division of labour.**
**Approving always happens in the Repotory desktop app (the workbench)** — that is where a human is
present. The copilot's memory panels are **read-only**.

### 3. ~~How automatic is "auto-discovering" the MCP client?~~ (settled: it launches by itself)

The original question: Phase 1 of the roadmap says "auto-discover the local Repotory MCP". The options are: find the `repotory-mcp` binary on PATH, a fixed path, or a user-configured one.

**How it works now**: it probes PATH plus the common install directories (`/usr/local/bin`,
`/opt/homebrew/bin`, `~/.local/bin`) and, on a hit, **launches the child process over stdio by
itself** — **exactly like the two IDE plugins** (VS Code / JetBrains both spawn `repotory-mcp`
themselves, and that path already works). It then does one handshake (`initialize` →
`notifications/initialized`), fetches the tool list with `tools/list`, and **stays resident** (the
server keeps the same SQLite database open, so re-opening it per call means re-opening the database
per call).

**Why launching by itself is right**: without a server there is no memory, and then the copilot has no
reason to exist; while "probe but never launch" makes the first screen say "installed" — **it is
promising something it never verified**. The first screen now tells three states apart: not installed /
installed but not answering / connected with the server's tool list.

**The half that is still open**: "fall back to plain chat when the connection fails" waits for Phase 2.
Today a failed connection makes the first screen say "not answering" and offer a retry — **it does not
pretend the memory is still there**.

### 4. Version coupling with Repotory

The MCP tool set the Agent consumes evolves with Repotory (`repotory_*` tools are added and removed). Does the Agent need a capability handshake ("which tools do you have?") instead of assuming a list, or is a semantic-version constraint on Repotory enough?

**Open**: without a handshake, the Agent↔Repotory version combination is an implicit constraint that has to be written down in the README and the installer (not just kept in someone's head).
**Already decided**: the tool list is **fetched dynamically** (see above), so a newly added tool is not
invisible just because a list was not updated — **a capability handshake solves "a tool was removed or
renamed", not "a tool was added"**.

### 5. Should the copilot perform writes at all?

What this repo fixes is "it goes over MCP, with one set of write tools" (`notes_create` / `notes_update` /
`notes_organize` / `handoff`). But **"should the copilot write notes" is a product decision, not a
technical one**.

**Open**: whether writes also require human confirmation (Phase 0's "human confirmation" capability currently covers command execution only).
