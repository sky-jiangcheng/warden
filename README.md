# Repotory Agent Shell

A local Agent entry point. The Shell is the window, [Repotory](https://github.com/sky-jiangcheng/repotory) is the memory, and MCP is the open protocol between them.

> **Status: Phase 0 — repository and positioning.** The app builds and runs; the chat loop lands in Phase 2.

## What this is

An Agent shell that runs on your own machine: open a repository, talk to a local model about that project, and let Repotory remember what matters.

The three roles stay separate:

| | Owner | What it owns |
|---|---|---|
| Entry point | Agent Shell (this repo) | Local model entry, memory panels, task confirmation |
| Memory | [Repotory](https://github.com/sky-jiangcheng/repotory) | Project knowledge, session handoffs, pending knowledge |
| Protocol | MCP | The Shell reads memory through `repotory_*` tools only — never the database |

## What runs today

- Detects the local model runtimes (Ollama first; llama.cpp / MLX reserved as pluggable backends)
- Detects whether the memory layer (Repotory MCP) is installed
- Shows the first-version product boundary and the milestones

Chat, command execution and the memory panels do not exist yet — see the [roadmap](docs/roadmap.md).

## Stack

Tauri v2, Rust backend, TypeScript frontend. Ollama is the first model entry point; the default model is `qwen3:8b` (Qwen3 8B Q4).

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
