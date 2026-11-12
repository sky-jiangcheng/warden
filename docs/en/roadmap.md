# Roadmap

> Source: the "总体路线" discussion draft of 2026-10-28. This page turns it into the executable version that lives in the repository.
> Positioning and boundary are in [product.md](product.md), which also records where the roadmap and Cinder's current state disagree.

## The shape

```
Cinder MCP memory layer     ↑ copilot (Agent, Tauri)             ↑ local models (Ollama / llama.cpp / MLX)
                             ↑ tool layer (Git / Shell / file index / database CLI)
                             ↑ mainstream clients (Claude Code / Cursor / VS Code / JetBrains)
```

The memory layer is open over MCP; the copilot itself only does "local model entry + command execution + memory panels".
Here **"copilot" means this product** (Warden) — **not** GitHub Copilot.

**It and the Cinder Desktop app are two products**: the desktop app is the **workbench** (browse,
edit, approve, govern), the copilot is the **assistant** (read, chat, execute). They share the memory
layer and the protocol — not the interface, and not the lifecycle.

## Phase 0: repository and positioning (1 week) — done

Split the copilot out of Cinder explicitly, making it **its own product**, while keeping it tightly
bound.

- Create the `warden` repository (formerly `repotory-agent-shell` → `repotory-agent`, renamed on 2026-11-04; renamed again to `sky-jiangcheng/warden` on 2026-11-05);
- Position it: an **independent copilot** that talks to Cinder MCP by default;
- Lock the stack: Tauri v2 + Rust backend + TS frontend;
- Ollama first as the model entry, llama.cpp / MLX later;
- Default model Qwen3 8B Q4, proven on an M4 Pro first;
- Define the first-version boundary: chat, read project, run commands, memory panels, human confirmation.

**Progress so far**: done. The repository and skeleton are in place, the positioning/boundary/decisions live in [product.md](product.md), and the first screen shows the detected memory layer, model runtimes, and the **wake-up entry (the tray)**.
The CI gates (docs mirror, frontend build, `cargo fmt` / `clippy` / `test`), the brand icon, and the tray (ADR-0047 decision 5) are all in place.

## Phase 1: wire up the MCP memory layer (1–2 weeks) — the data path is done

Make the copilot consume Cinder's memory reliably.

- Confirm these MCP tools are stable: `repotory_context`, `repotory_handoff`, `repotory_notes_search`, `repotory_projects_list`, `repotory_scan` (the original roadmap's `repotory_pending` and `repotory_snapshot` do not exist in Cinder — see [product.md, open question 1](product.md#1-repotory_pending-and-repotory_snapshot-do-not-exist-in-cinder));
- **Fetch the tool list dynamically**: use MCP's `tools/list` in place of the hardcoded copy (it has
  already drifted — see [product.md, the memory-layer contract](product.md#the-memory-layer-contract));
- Implement the MCP client on the Agent side (**copy the path the two IDE plugins already walk**:
  spawn the `repotory-mcp` child process over stdio):
  - auto-discover the local Cinder MCP;
  - check the connection at startup;
  - degrade to plain chat when it fails (**the first screen has to say "no `repotory-mcp`, no memory" — no pretending**);
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
- Attach context retrieved from Cinder;
- Every model request goes through Rust; no port is exposed to the frontend.

Goal: open the copilot, pick a repository, talk to a model about the project.

**Progress so far**: **the chat loop works and the model entry is open**.

- **The provider configuration layer** (`src-tauri/src/providers.rs`): a provider is "a base URL".
  The configuration lives in `providers.json` in the app config directory, and the **API key lives in
  `secrets.json` beside it, created `0600`** — the config file holds only whether a key exists. Not the
  OS credential store: early builds were ad-hoc signed, so a keychain ACL had no signature to bind to
  and "always allow" had to be re-answered on every build (the v0.1.7 decision; the reasoning is the header
  comment of `src/providers.rs`); the release pipeline signs with a real Developer ID now, and the file stays;
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

**Still not done**: injecting the git diff, and writing the answer back with `repotory_handoff`; the
repository path is already in the system prompt and the read-only tool face (`repotory_metrics` and
its seven siblings) is already wired into the conversation. The notion of "the current repository"
landed in Phase 2.5, so these two remain **wiring, not a blocked prerequisite**.
The layered policy for what may be written is **settled** (see "Open questions" item 5 in
[product.md](product.md)): `handoff` / `scan` are written automatically, while **durable knowledge
(`notes_create` / `notes_update` / `notes_organize`) needs a human confirmation first**.

## Phase 2.5: finding the repositories itself (1 week) — done

The copilot goes from "one front end of Cinder" to "an independent robot on this machine".

- **It walks the disk itself**: git repositories under the roots the user configured
  (a `.git` *directory* counts; a worktree's `.git` *file* does not), **without going
  through Cinder's scanner and without reading its database**;
- **A list, and the user decides**: the repositories are listed, and the user says which
  ones are theirs;
- **Follow or dismiss**: two answers. "Follow" means "this is my work"; "dismiss" means
  "stop asking". **A dismissed repository never surfaces again** — otherwise saying "no"
  once would still be asked a second time;
- **A bubble for new repositories**: one repository at a time (five cards at once is a
  form, not a question). The bubble slides down from the top of the window;
  **and that is the only channel** — no system notification, no notification action
  (see engineering gap 2);
- **It keeps looking while nobody is looking**: the scan runs every five minutes even
  with the window closed. That is what makes it a robot rather than a report.

**The watchlist is the copilot's own** (`repos.json` in the app config directory) and
**does not go into Cinder's database**: the writer of `projects.is_starred` there is the
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
approving always happens in the Cinder Desktop app.

## Phase 5: local model management (1–2 weeks)

- Detect whether Ollama is installed;
- Installation guidance when it is not;
- Pull the default model automatically;
- Switch models;
- Show model state, latency, VRAM/RAM usage;
- Offline-mode hints.

llama.cpp / MLX would arrive through the same "one base URL" route — **never a per-vendor branch**:
a local provider and a remote provider are the same thing. The `model_backends.rs` registry was
deleted in Phase 2 (see "Phase 0 decisions" in [product.md](product.md)).

## Phase 6: distribution and shipping (1–2 weeks)

- macOS dmg packaging;
- Optional Homebrew cask;
- The App Store build ships no model and points at Ollama;
- A `repotory-agent init` one-shot initializer;
- A README that says it plainly: the copilot is the entry point, Cinder is the memory, MCP is the
  open protocol;
- **The version coupling written down**: which Agent version pairs with which **memory-layer contract
  version** (the contract version is the main repo's `internal/version/core.go` `CoreVersion`; it
  answers "did the tool surface change" — without a capability handshake that coupling is implicit —
  see [product.md, open question 4](product.md#4-version-coupling-with-cinder)).

## Milestones

| Milestone | Done when | State |
|---|---|---|
| M1 | It can call Cinder MCP | done (Phase 1) |
| M1.5 | **It can find the repositories itself and let the user say which ones matter** | **done (Phase 2.5)** |
| M2 | It can talk to a model about the current repository | in progress (Phase 2: the chat loop works; the repository path is in the system prompt and the read-only tool face is wired; diff injection and the handoff write-back are not) |
| M3 | It can safely run Git / Shell commands | not started |
| M4 | It can see and manage cross-session memory | not started |
| M5 | It installs and initializes in one step | not started |
| M6 | It ships as a standalone product | not started |

## Engineering gaps (outside the milestones, 2026-11-11 review)

These three belong to no phase, yet all of them block "releasable". Close them against the
acceptance criteria; no new process.

### 1. Frontend state-machine tests — closed (2026-11-12, option 1 of "open question 6")

- **Problem**: `npm run build` is `tsc` only; none of the three CI gates runs a frontend test.
- **Impact**: `src/chat.ts`'s revision dropping, concurrency, IME guard and draft retention are
  hand-tested; one change to the chat flow can regress them.
- **Acceptance**: land the choice from "open question 6" in [product.md](product.md) and wire it into CI.
- **How it was closed**: the four decisions were extracted to `src/chat-state.ts` as pure functions,
  and `chat.ts` calls them — so the tests exercise the production path, not a copy that can drift.
  vitest cases cover all four acceptance paths (`src/chat-state.test.ts`, `npm run test:fe`), and CI
  gained a `frontend-test` job. The DOM bindings and the `Channel` transport are deliberately
  untested; see the reasoning in open question 6.

### 2. System notification: promised in docs, absent in code — closed (2026-11-12, the "remove" side)

- **Problem**: Phase 2.5 and the README promised "when the window is in the background it becomes a
  system notification" (corrected 2026-11-11), but `builder()` (`src-tauri/src/lib.rs`) registers
  only opener and global-shortcut; `tauri-plugin-notification` sits in `Cargo.toml` with no
  `.plugin(...)`, no capability and no call site. The scan-payload test asserts the opposite: no
  announcement channel.
- **Impact**: a dependency carried for nothing (bundle size, permission audit) and a wake-up path
  the docs promise but the app cannot deliver.
- **Acceptance**: pick one — remove the dependency and its rationale comment, or add registration,
  permission and a test and restore the docs; docs and code in the same commit.
- **How it was closed**: the docs and the tests both stand on "no system notification" — the
  in-window bubble is the only channel — so removing the dependency is the fix that matches the
  product decision. `tauri-plugin-notification` is gone from `Cargo.toml` together with its
  rationale comment, replaced by a comment explaining why it is absent; `Cargo.lock` follows.
  If a notification path is ever built: registration, capability, test and restored docs, all in
  the same commit.

### 3. Put macOS in CI — closed (2026-11-12)

- **Problem**: the Rust gates run on `ubuntu-latest`; the tray (`muda` main thread), the Accessory
  policy, the template icon and `tauri build` packaging only mean anything on macOS.
- **Impact**: pre-release verification is manual, and "is the tray there" is self-reported through
  `tray.installed` only.
- **Acceptance**: a macOS CI job (fmt + clippy + test + `tauri build --debug` smoke) whose failure
  says whether compile, tests or packaging broke; signing not required.
- **How it was closed**: `ci.yml` gained a `rust-macos` job (`macos-14`): fmt / clippy / test run
  the same commands as the Ubuntu job, plus a packaging smoke, `tauri build --debug --bundles app`
  — four steps, one concern each, so a red run says whether compilation, tests or packaging broke.
  The smoke builds the `.app` only, not the dmg: `hdiutil` needs disk-image privileges the runner
  may or may not grant, and that flakiness would make the gate untrustworthy while proving nothing
  the `.app` does not (same binary, icon ladder and plist inputs). No signing secrets are passed,
  so Tauri falls back to ad-hoc signing — enough to show the packaging graph resolves. Signing,
  notarisation and the dmg stay where they were: the tag-triggered release workflow.

## The trade-off

Do not turn the copilot into a "full IDE" on day one. Make it a good local assistant first: open it and chat about the project, see the memory, run commands safely, continue where the last session stopped. Once that path works, consider Claude Code, Cursor, VS Code and JetBrains — or turning it into a more general local Agent platform.

**The tray is in place** (ADR-0047 decision 5): it is the carrier of "close the workbench and
an assistant is still there". The tray icon is the copilot's wake-up entry — click it to bring the
window back, right-click for the menu (Show / Quit).
**Closing a window hides it; only Quit in the tray exits**: `on_window_event` does
`prevent_close` + hide on `CloseRequested`, keeping both webviews and their in-flight chat tasks
alive — the red button must not cut off an answer (consistent with "the resident shape" in [product.md](product.md)).
**⌥ Space is registered** (`SHORTCUT` in `src-tauri/src/tray.rs`), and **the always-visible companion
window has landed too**; **the exact shape is still decided by user feedback**.
