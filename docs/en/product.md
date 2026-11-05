# Positioning and boundary

> This page settles three things for Phase 0: what this assistant is, how far the first version goes, and which decisions are left to the maintainer.
> The phases and milestones live in [roadmap.md](roadmap.md).

## One line

**Warden is an independent product: a local copilot that runs on your own machine (**not** GitHub Copilot) — not another Agent platform, and not one more screen inside the Cinder Desktop app.** It turns "talk to a model about the project in front of you" into a window, and leaves "what to remember" to Cinder.

**It and Cinder Desktop are two products, one job each**: Cinder Desktop is the **workbench** (browse, edit, **approve**, govern), Warden is the **assistant** (read, chat, execute). They share one local memory and one protocol, and **approval stays in Cinder Desktop**.

## Three roles, three hard boundaries

| Role | Owner | Owns | Does not own |
|---|---|---|---|
| copilot | Agent (this repo) | **Finding the repositories on this machine and the user's watchlist**, model entry, command execution, memory panels | Retrieval ranking, project knowledge storage, **approval** |
| memory | [Cinder](https://github.com/sky-jiangcheng/repotory) | Project context, session handoffs, approved knowledge pages | Deciding what the copilot looks like |
| protocol | MCP | The only channel between the Agent and the memory layer | —— |

The three boundaries are hard:

1. **The Agent does not re-implement Cinder's retrieval.** Context comes from `repotory_context`; it does not rank FTS itself, does not compute embeddings itself, and keeps no vector store of its own.
2. **The Agent does not touch Cinder's database.** MCP tools only. The database is Cinder's private implementation, and the Agent has no business knowing it exists.
3. **The Agent is not a full IDE.** Routing belongs to Cinder (`repotory_notes_read` / `repotory_notes_update` and friends); the Agent only consumes it.
4. **The Agent finds the repositories itself, without borrowing Cinder's hands.** Walking the disk is the copilot's own job: it looks for git repositories under the roots the user gave it, lists them, and remembers which ones the user cares about. **The repository list does not go into Cinder's database** — that database is a knowledge base, not the register of "what the user is working on".

## How it relates to the Cinder Desktop app: two products, one job each

**Not master and servant, not two interfaces of one product.**

| | Cinder Desktop | Agent (the copilot) |
|---|---|---|
| Positioning | the **workbench** | the **assistant** |
| What it does | browse, edit, **approve**, govern | read, chat, execute |
| Memory channel | uses `internal/service` directly | **MCP only** |
| Approval (approve / reject) | **yes** (a human is present) | **no** — those two tools are not on the MCP surface at all |
| Closing it | the workbench is gone | the copilot keeps running |
| Where the repositories come from | its own scan roots (`scan_roots`) | **its own scan roots + its own watchlist** |
| Interface | the three-column shell + the small bot float | its own window + a tray |

**Approval stays with the human, and not by convention.** Cinder's MCP surface registers only two
**pure-read** knowledge page tools (`repotory_wiki_search` / `repotory_wiki_read`), because the gate on
the write side is "publishing content into retrieval is a human act" (ADR-0015 decision 3).
**The Agent goes over MCP, so it stands outside that gate automatically — it cannot cross it even if it
wanted to.**

**The Agent needs the `repotory-mcp` binary to exist before it has any memory.**
**No `repotory-mcp` binary, no memory** — and that binary **ships as a standalone release asset**
(one per platform), so the **desktop app is not required**; the first screen has to say so plainly
rather than pretending.

## The first-version boundary

Five capabilities, fixed by Phase 0, plus one more ("find the repositories") added by Phase 2.5:

| Capability | Means | Done when |
|---|---|---|
| **Find repositories** | **Walk the disk, list the repositories, and let the user say which ones matter** | **A new repository surfaces as a bubble; a followed one lands in the list** |
| Chat | Talk to the model about the current project, streamed | Pick a repository + ask a question → get an answer |
| Read project | Attach the repository path, the recent git diff, context retrieved from Cinder | Every attachment is selectable and previewable |
| Run commands | Whitelisted tool execution; writes need human confirmation | The full command is shown, dangerous ones are blocked by default, every run is logged |
| Memory panels | Current context / approved knowledge / cross-session conclusions | All three panels show real data |
| Human confirmation | Dangerous actions never run unattended | The user sees the full command and its blast radius before confirming |

**"Human confirmation" covers two things**: whitelisted command execution (Phase 3), and the copilot
writing **durable knowledge** over MCP (`notes_create` / `notes_update` / `notes_organize`) — see item 5
under "Open questions" below.

## Decisions fixed in Phase 0

| Decision | Content | Cost |
|---|---|---|
| Stack | Tauri v2, Rust backend, TypeScript frontend | Not the same stack as the Cinder Desktop app (Wails/Go). The two share data and protocol, nothing else |
| **Why Tauri** | **it ships a tray** (menu-bar icon, click to wake) — Wails v2 has no tray (the `TrayMenu` in `pkg/menu` is a dead type, and the external `systray` needs cgo). **The tray is in place** (`src-tauri/src/tray.rs`): left click wakes the window, right click opens the Show / Quit menu | **Tauri uses the system WebView, not Swift/AppKit native controls** — it is "an independent process + a real tray + a real distribution channel", **not "a native UI framework"** |
| Model entry | **Any provider that conforms to the OpenAI-compatible API**, no vendor hard-coded | One client covers them all; local and hosted take the same road |
| Default provider | the local Ollama (`http://127.0.0.1:11434/v1`) — no account, no key | Another model means configuring another provider |
| Default model | `qwen3:8b` (Qwen3 8B Q4), proven on an M4 Pro first | Switching models means the user pulls it in Ollama |
| Memory channel | MCP, never a private store | The Agent depends on Cinder's MCP binary being present |
| Finding repositories | **the copilot walks the disk itself**; the user gives roots and decides what to follow | **the watchlist and Cinder's database are two separate stores** — sharing them means changing the main repository, which is not a copilot's job |

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

Every MCP tool the Agent consumes comes from Cinder's existing `repotory_*` tool set (the Cinder repository is the source of truth):

- `repotory_context` — project context; Phase 1's main data source
- `repotory_handoff` — writes the session handoff; Phase 1's exit
- `repotory_notes_search` / `repotory_notes_read` / `repotory_notes_create` / `repotory_notes_list` / `repotory_notes_update` / `repotory_notes_organize` — note reads and writes
- `repotory_projects_list` / `repotory_projects_stats` — the project list
- `repotory_wiki_search` / `repotory_wiki_read` — **approved** knowledge pages (pure reads)
- `repotory_scan` — discovering local Git repositories
- `repotory_ask` — evidence-backed answers with sources (ADR-0033)
- `repotory_agent_score` / `repotory_integrity` — introspection

The tool names' single source of truth is the Cinder main repository, and **this repository keeps no
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
> Cinder adds a tool, the copilot sees it without a line of code changing here.

**Beyond the tool surface, this contract has a version number too**: ADR-0048 in the main repo gives
the memory layer its own **contract version** (`internal/version/core.go`'s `CoreVersion`), and that is
what the MCP `initialize` response reports as `serverInfo.version`. **It answers "did the tool surface
change", not "is the build new"** — the main repo pins that number next to the tool list and holds it
with a CI gate (changing the tool surface forces a version bump), so the implicit "which version pairs
with which" constraint **has become a number you can read** on that side. **This repo keeps no copy of
it** (same rule as the tool list) — it reads the server's value when it connects.

## Open questions (the maintainer decides)

### 1. ~~`repotory_pending` and `repotory_snapshot` do not exist in Cinder~~ (clarified)

Two of the four tools the original roadmap listed for Phase 1 are not there: `repotory_context` and `repotory_handoff` are real, but `repotory_pending` and `repotory_snapshot` have no counterpart in Cinder's MCP tool set.

- "Pending knowledge" in Cinder is really the wiki page status `pending / approved / rejected` (`internal/db/wiki.go`);
- A snapshot is Cinder's local backup mechanism (ADR-0025 L1) — an internal capability, not an MCP tool.

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
**Approving always happens in the Cinder Desktop app (the workbench)** — that is where a human is
present. The copilot's memory panels are **read-only**.

### 3. ~~How automatic is "auto-discovering" the MCP client?~~ (settled: it launches by itself)

The original question: Phase 1 of the roadmap says "auto-discover the local Cinder MCP". The options are: find the `repotory-mcp` binary on PATH, a fixed path, or a user-configured one.

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

**Settled (2026-11-05): the chat continues, and a banner says so out loud.** When the memory layer
cannot be reached the chat still works, but the interface has to say "this turn has no memory", and
**a failed context assembly must never be swallowed** — a silent fallback means the model answers with
no basis while the user cannot tell that this turn had none. **Today the shape is already this**: the
chat and the memory layer are two independent roads (`chat_send` does not depend on the MCP state), and
the memory layer is one card among several. What this decision really constrains is the behaviour
**after context injection lands** — not "can we chat", but "does this turn have a basis, and did we
say so".

### 4. ~~Version coupling with Cinder~~ (settled: self-evident at runtime, warn in plain words)

The original question: the MCP tool set the Agent consumes evolves with Cinder (`repotory_*` tools are added and removed). Does the Agent need a capability handshake ("which tools do you have?") instead of assuming a list, or is a semantic-version constraint on Cinder enough?

**Settled (2026-11-05): self-evident at runtime, warn in plain words, never hard-refuse.**

① **The server's version was already in hand** — Cinder's MCP server reports its `serverInfo` in the
`initialize` response (main repo, `cmd/mcp/main.go` — `server.NewMCPServer("repotory-mcp", version.CoreVersion)`),
and **this repo threw the whole response away** (`mcp_client.rs:250-259` only `?`s the result, it never
binds it). Catching it is one change, **not a new protocol**.
② **The first screen proves it**: it shows "**memory layer vX.Y.Z · N tools**" — the version and the tool count
both come from the server, and **this repo keeps no copy** (the same rule as the tool list).
**That X.Y.Z is the contract version** (the main repo's new `internal/version/core.go` `CoreVersion`,
ADR-0048): **it answers "did the tool surface change", not "is the build new"** — the release version
is on `/health` and in the startup log, but it has **no answer** to "what may I rely on" (the interface
changes daily, the tool surface moves every few months, and a 2.30.0 cannot say whether the tool
surface changed). **So the first screen says "memory layer vX.Y.Z", not "Cinder vX.Y.Z".**
③ **Warn only when out of bounds, never block**: below the supported floor the app says one plain
sentence ("this memory layer is too old — upgrade it to use memory") and **does not hard-refuse** — a
hard refusal turns an implicit coupling into an explicit one that still stops people, while the dynamic
tool list already covers the "a tool was added" half.
④ **No real capability handshake** (the client declares the tools it needs and probes for the missing
ones): that solves "a tool was removed or renamed", which is rare, at a cost far above the benefit —
**that half waits for the day it actually happens**.
⑤ **The floor is derived from the contract version's major number**, not from a magic number copied by
hand — the main repo pins the contract version **next to the tool list** (`wantCoreVersion` in
`cmd/mcp/main_test.go`) and its CI gate guarantees "changing the tool surface forces a version bump",
so the number this repo reads **marks the floor itself**: backwards compatible within one major,
re-negotiable only across a major.

**Already decided**: the tool list is **fetched dynamically** (see above), so a newly added tool is not
invisible just because a list was not updated — **a capability handshake solves "a tool was removed or
renamed", not "a tool was added"**.

### 5. ~~Should the copilot perform writes at all?~~ (settled: layered — session output automatic, durable knowledge gated)

The original question: what this repo fixes is "it goes over MCP, with one set of write tools" (`notes_create` / `notes_update` /
`notes_organize` / `handoff`). But **"should the copilot write notes" is a product decision, not a
technical one**.

**Settled (2026-11-05): a layered gate.**

① **The MCP write surface is five tools**: `notes_create` / `notes_update` / `notes_organize` /
`handoff` / `scan` (`ai_review_rules` is read-only in practice). The other 13 are reads.
② **Written automatically**: `handoff` (the session handoff — it was designed for agents to write, and
ADR-0040 already put it in the evidence layer) and `scan` (registering a repository is not knowledge).
③ **Confirmed by a human first**: `notes_create` / `notes_update` / `notes_organize` — **a note is
retrievable the moment it is written** (`SearchAll` covers notes), so the copilot could write into
**the very memory it is about to read next**; ADR-0015's own words describe exactly that shape
("model writes into the store with no gate"). **A session output and durable knowledge are not the
same kind of write**: the former is automatic, the latter goes through the gate.
④ **When it lands**: the copilot **calls no MCP tool at all today** (it only connects and lists), so
this applies when M2 actually writes the answer back through `repotory_handoff`.
⑤ **Phase 0's "human confirmation" now covers two things**: whitelisted command execution (Phase 3),
and the copilot writing durable knowledge over MCP (the three above).
