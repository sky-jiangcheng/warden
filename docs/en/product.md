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
| memory | [Cinder](https://github.com/sky-jiangcheng/cinder) | Project context, session handoffs, approved knowledge pages | Deciding what the copilot looks like |
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
| Run commands | Whitelisted tool execution; writes need human confirmation (**Phase 3 has not started**, see [roadmap.md](roadmap.md) M3) | The full command is shown, dangerous ones are blocked by default, every run is logged |
| Memory panels | Current context / approved knowledge / cross-session conclusions | All three panels show real data |
| Human confirmation | Dangerous actions never run unattended (**Phase 3 has not started**) | The user sees the full command and its blast radius before confirming |

**"Human confirmation": one half settled, one half waits for Phase 3**: the copilot
writing **durable knowledge** over MCP (`notes_create` / `notes_update` / `notes_organize`)
is settled (see item 5 under "Open questions" below); whitelisted command execution
belongs to **Phase 3, which has not started** ([roadmap.md](roadmap.md) M3 is marked "not started").

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

**The API key lives in `secrets.json` in the app config directory, created `0600`** (not in the OS
credential store: early builds were ad-hoc signed, so a keychain ACL had no stable signature to bind to
and "always allow" had to be re-answered on every build; the release pipeline signs with a real
Developer ID now, and the file stays anyway — no prompt, readable and deletable). The provider config
file holds only whether a key exists.

**Everything the user is allowed to have a preference about** — which model, the shape of the
interface — is left to the user.

**The wake-up entry is in place, but the overall shape of the wake-up interaction is still undecided**: the tray icon is the current entry (left click wakes the window, right click opens the menu), and the first screen carries a "wake-up entry" card that shows in real time whether the tray is there — macOS does not let an external process query somebody else's menu-bar status item, so "is the tray actually there" can only be answered by the app itself (`tray.installed`), and when the tray fails to install the log carries a `[tray] ...` line.
**The ⌥ Space global shortcut is registered** (`SHORTCUT` in `src-tauri/src/tray.rs`), and **the always-visible companion window has landed**
(`index.html?surface=companion`, `alwaysOnTop`); whether the shape is adjusted further waits for user feedback.

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
- `repotory_ai_audit` — what this machine sent to an AI model recently: when, which endpoint, which model, which surface, how many characters, and whether a local review rule blocked or flagged it
- `repotory_ai_review_rules` — the local AI review rules and the input/output review switches (**read-only**)
- `repotory_chat` — the **shared conversation**: threads and messages read and written across clients (`list` / `create` / `read` / `append` / `rename`); Warden, VS Code and JetBrains look at **the same record**
- `repotory_metrics` — **the numbers**: repository and knowledge counts already stored in memory (it does not scan disk, run git or refresh caches)

The tool names' single source of truth is the Cinder main repository, and **this repository keeps no
runtime copy of that list**. Phase 1 deleted that copy: the tool list is fetched dynamically via MCP's
`tools/list` at connection time (`src-tauri/src/mcp_client.rs`), and what the first screen shows is what
the server just reported.
**Phase 0's copy listed 7 names while the server registered 18 at the time** (20 today, see below) — and the missing ones included
`repotory_wiki_search` / `repotory_wiki_read` (the only way to read approved knowledge) and
`repotory_ask` (evidence-backed answers).
**A list that drifts is worse than no list, because it makes people think it is real** — there is now a
self-check test in `memory_layer.rs` that watches for tool names being written back into the source.

> The list above is **for readers, not a runtime contract**: on this machine the server reports **20**
> tools, matching the names listed above. **At runtime the server's answer always wins** — when
> Cinder adds a tool, the copilot sees it without a line of code changing here.

**Beyond the tool surface, this contract has a version number too**: ADR-0048 in the main repo gives
the memory layer its own **contract version** (`internal/version/core.go`'s `CoreVersion`), and that is
what the MCP `initialize` response reports as `serverInfo.version`. **It answers "did the tool surface
change", not "is the build new"** — the main repo pins that number next to the tool list and holds it
with a CI gate (changing the tool surface forces a version bump), so the implicit "which version pairs
with which" constraint **has become a number you can read** on that side. **This repo keeps no copy of
it** (same rule as the tool list) — it reads the server's value when it connects.

## The resident shape, conversation-as-interface and cross-client sync (landed 2026-11-06)

The user's positioning of this product is three sentences: **a small resident robot**, **a fast look at
numbers like lines of code**, and **a topic-switching chat panel that stays in sync with the IDE
plugins**. All three are landed; each has its own boundary.

**① Resident = the companion window + the tray, not "the window is always on screen"**. The companion
window is 132×128 (`index.html?surface=companion`), the main window 480×760; clicking the companion
opens the chat. The desk pet is a white cat ("霜", Frost): a body layer and a head layer cut with a fur
overlap (the seam hides in identical fur), eyes that follow the pointer anywhere on screen (the head
turn fades with distance, eased per-frame), self-timed blinks, and a tsundere sulk when left alone;
reactions like a click move the whole cat while the head only tilts about the neck, so no seam opens. **The window holds the cat and one speech bubble, nothing else**: the metric line,
the status line and the "chat" button are gone — numbers belong to the conversation (see ②), and the
entrances are click / `⌥Space` / the tray, so a few lines of text on top of a character were just another
layer of interface. **Only the cat body starts a
native window drag** (`window_drag`); no control carries `data-tauri-drag-region` — an illustration made
into a button is a button the user cannot press, and they will blame the program. **Closing the window
hides it; only the tray's Quit exits**: `on_window_event` answers `CloseRequested` with `prevent_close`
plus a hide, and the reason written in the code is "keep both webviews and their in-flight chat tasks
alive" — **the red button must not cut off an answer that is being generated**. Warden is also the
parent process of `repotory-mcp`, so exiting would tear down that conversation channel with it.
**Cinder Desktop does the opposite and already exits on close** (ADR-0047 amendment): it hosts no
long-lived channel.

**② Conversation as the interface: the numbers come from tool calls in the chat, and the main window
no longer carries a resident panel** (landed the same day). The model request now carries `tools` — a
read-only whitelist filtered live from the server's `tools/list` (`repotory_metrics` /
`repotory_context` / `projects_list` / `projects_stats` / `notes_search` / `notes_read` /
`wiki_search` / `wiki_read`), **with the schemas passed through verbatim from the server**; a model
that names a tool outside the whitelist (including write tools the server advertises) gets a
**client-side refusal**, never an execution; `repotory_chat` is not in the model's tool face at all —
**persisting the reply is Warden's own job, and a model that could append would double-write the very
conversation it is answering**. The round cap is 5, after which one final text-only round is forced.
A tool call shows in the conversation as a "calling / done" status line only, **never written into
`repotory_chat`** — it is a UI-layer event, not a conversation artifact. **A model tool's answer
travels verbatim; the client invents no shape** (added 2026-11-10): the server's tool face is not one
uniform JSON contract — `repotory_context` answers with a plain-text briefing, `repotory_projects_list`
with a JSON array, the rest with objects, and an argument-validation refusal is a plain sentence.
Warden used to parse every answer as JSON, so a healthy Cinder was reported to the model as
`returned invalid JSON` — **the model reads prose and JSON alike; parsing for it is copying a contract
that will drift**. Only the internal `repotory_chat` path still requires a JSON object: every one of its
answers is read by field (`threads` / `thread.id` / `messages`), and anything else is a broken server.
**The two project lists are
named apart in the system prompt** (added 2026-11-10): Warden's own watchlist (the directories the user
adopted from the disk scan, read from `repos.json` at send time) and Cinder's starred projects (what
`repotory_projects_list` answers) reply to two different questions — "what am I working on" versus "what
has the knowledge base governed" — and a model shown only one of them will answer "which repositories do
I watch" in the wrong register. When the watchlist is empty the passage does not appear at all: no list
is invented. A temporary conversation
without Cinder carries no tools and no system prompt; its single-turn contract is unchanged.

**③ The numbers are still asked for in the conversation, never computed on the spot, and they no longer
hang on the companion window**. To see lines of code or commits, let the model call `repotory_metrics` —
one tool call, with no resident metric line polled every 15 seconds. **That reading is deliberate**: the tool **does not
scan disk, run git or refresh caches**, so the numbers come fast and a question cannot change anything
stored. `totalLines` is a number only when the cached language LOC covers the whole unambiguous scope —
**otherwise it is null: better to show "there is no such number" than show a guessed one**.

**④ What syncs across clients is one and the same conversation, not two records that look alike**.
The topic list and the messages are each polled every 2.5 seconds from those two new tables in memory
(`chat_threads` / `chat_messages`); Warden, VS Code and JetBrains each keep a long-lived connection to
`repotory-mcp` and read and write the same rows. **Three hard constraints**:

- **Only completed messages sync**. A half-finished stream is not stored — **cross-client token streaming
  means pushing every token through a stdio pipe into another process's UI, which does not buy
  "in sync", it buys "frequently out of sync"**. So the status line says "synced / not synced",
  **never "typing"**.
- **Messages are immutable**. A message id is a client-generated UUID (an assistant reply uses
  `${uuid}-reply`): replaying the same id with the same payload is **idempotent**, the same id with a
  different payload **errors and never overwrites**. **Three clients appending to one thread concurrently
  can only be kept honest by forbidding edits and allowing appends.**
- **Sources are labelled truthfully**. Whether an assistant message reads "Warden" or "assistant · vscode"
  comes from the message's own `source` field — **labelling a reply written back by a plugin as Warden is
  claiming another client's answer**.

A conversation with no shared topic and no history is a **temporary chat**: it stays in this window only,
and the status line marks it as temporary and unsynced — **it does not pretend to be in the shared
layer**. Chat keeps working when the memory layer is unreachable, consistent with decision 3 above.

**⑤ The frontend opens no port**. Every sync is `invoke → Rust → MCP`; there is not one `fetch` to a
local port anywhere in `src/`. This is not a style preference: **"the Agent goes over MCP and touches no
other channel" is one of the hard boundaries above**, and a shared conversation is no reason to become
the first exception — one more channel is one more place that can answer "who is writing this memory".

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
`/opt/homebrew/bin`, `~/.ollama/bin` — `find_binary` in `src-tauri/src/binaries.rs` is the truth),
and, on a hit, **launches the child process over stdio by
itself** — **exactly like the two IDE plugins** (VS Code / JetBrains both spawn `repotory-mcp`
themselves, and that path already works). It then does one handshake (`initialize` →
`notifications/initialized`), fetches the tool list with `tools/list`, and **stays resident** (the
server keeps the same SQLite database open, so re-opening it per call means re-opening the database
per call).

**Why launching by itself is right**: without a server there is no memory, and then the copilot has no
reason to exist; while "probe but never launch" makes the first screen say "installed" — **it is
promising something it never verified**. The first screen now tells three states apart: not installed /
installed but not answering / connected; when connected it shows the contract version the server
reported plus the `tools/list` tool list (names and descriptions both come from the server), and the
chip reads "记忆层 vX.Y.Z · N 工具".

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

① **The MCP write surface is six tools**: `notes_create` / `notes_update` / `notes_organize` /
`handoff` / `scan` / `chat` (`ai_review_rules` is read-only in practice). The other 14 are reads.
② **Written automatically**: `handoff` (the session handoff — it was designed for agents to write, and
ADR-0040 already put it in the evidence layer), `scan` (registering a repository is not knowledge) and
`chat` (appending a session message — a **session output**, and the tables `repotory_chat` writes
**never enter retrieval**, so this write cannot influence what the copilot reads next; it belongs with
②, not with ③).
③ **Confirmed by a human first**: `notes_create` / `notes_update` / `notes_organize` — **a note is
retrievable the moment it is written** (`SearchAll` covers notes), so the copilot could write into
**the very memory it is about to read next**; ADR-0015's own words describe exactly that shape
("model writes into the store with no gate"). **A session output and durable knowledge are not the
same kind of write**: the former is automatic, the latter goes through the gate.
④ **When it lands**: the copilot's own by-name bindings are still two — `repotory_chat` (the shared
conversation transport) and `repotory_metrics` (the numbers the chat asks for); **the model**, in
conversation, can call the eight read-only whitelisted tools (see "the resident shape" ②), all
read-only, so none of this touches the gate; the three durable-knowledge writes are **refused
client-side** for the model and will never execute. This applies when M2
actually writes the answer back through `repotory_handoff`.
⑤ **Phase 0's "human confirmation": one half settled, one half waits for Phase 3** — the copilot
writing durable knowledge over MCP (the three above) is settled; whitelisted command execution
belongs to **Phase 3, which has not started** ([roadmap.md](roadmap.md) M3).

### 6. ~~How the frontend state machine gets tested~~ (settled 2026-11-12: option 1, pure functions + vitest)

**The original question**: `src/chat.ts` / `src/main.ts` carry the state machine — dropping stale
revisions, one send at a time, no send while an IME is composing, keeping the draft after a failure,
de-duplicating the 2.5-second polls — and **none of it has an automated test**. `npm run build` is a
type check, and none of the three CI gates runs a frontend test.

**Settled**: option ① — the decisions were extracted to `src/chat-state.ts` as pure functions
(`sendBlocked`, `readIsStale`, `shouldAdoptThread`, `draftAfterSend`), tested directly with vitest
(`src/chat-state.test.ts`, `npm run test:fe`, its own CI job). `chat.ts` calls these functions, so
the tests exercise the production path, not a copy that can drift. The DOM bindings and `Channel`
transport stay untested on purpose: they are the parts a type check plus the real app smoke cover
cheaply, while the four silent-regression rules needed a gate.

**Acceptance — met**: four paths covered, concurrent sends to one thread keep only the last
(`sendBlocked` on `busy`), a stale revision is dropped (`readIsStale`, including the busy case),
no send during IME composition (`sendBlocked` plus the shared `COMPOSITION_GRACE_MS` the keydown
guard uses too), and the draft survives a failed request (`draftAfterSend` on `failed`).

### 7. What the "human confirmation" looks like on the copilot side when durable knowledge is written back (open)

**Settled** (item 5): `notes_create` / `notes_update` / `notes_organize` need one human confirmation;
approving or rejecting wiki pages always happens in Cinder Desktop, whereas this is the confirmation
for the copilot's own durable writes.

**Open**:
① whether confirmation happens **before** the tool call or **after** the result comes back — the
former can prevent the write, the latter can only undo it;
② whether it is a card in the conversation or a separate panel;
③ where the record goes (a local audit log, or a table in the memory layer that never enters retrieval);
④ whether a timeout or a closed window means "refuse" or "park it for next time".

**Acceptance**: after the model asks for one `notes_create`, an unconfirmed call never executes and
leaves no new record in the memory layer; both confirm and refuse leave one traceable record
(who, when, which tool, which parameters).

### 8. Collapse errors from `String` into a structured type (open)

**The original question**: Tauri commands and the MCP / LLM clients all return `Result<T, String>`,
so the frontend can only print the text and cannot branch on the class of failure (not installed /
unreachable / auth failed / timeout / contract mismatch). Logs are `eprintln!` with no fields to query.

**Open**: which layer and how coarse.
① A `thiserror` enum inside the clients, flattened back to `String` at the IPC boundary (small change,
small frontend gain);
② An IPC shape of `{ code, message }`, with the frontend branching on `code` and showing `message`
(touches every command and call site);
③ Swap logging too (`tracing` with fields) — the largest change, paid back only in debugging.

**Acceptance**: the frontend takes a different action per class ("not installed" shows install
guidance, "auth failed" opens settings), the codes have tests, and logs can be filtered by
code / provider / thread.
