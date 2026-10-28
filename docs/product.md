# 定位与边界

> 本页定死 Phase 0 的三件事：这个 Shell 是什么、第一版做到哪、以及哪些决定要留给维护者拍板。
> 路线（Phase 与里程碑）在 [roadmap.md](roadmap.md)。

## 一句话定位

**Repotory Agent Shell 是本地 Agent 的入口，不是又一个 Agent 平台。** 它把"跟本地模型聊当前项目"这件事做成一个窗口，把"记得什么"交给 Repotory。

## 三个角色，三条边界

| 角色 | 归属 | 负责 | 不负责 |
|---|---|---|---|
| 入口 | Agent Shell（本仓库） | 本地模型入口、记忆面板、任务确认 | 检索排序、项目知识存储 |
| 记忆 | [Repotory](https://github.com/sky-jiangcheng/repotory) | 项目上下文、会话交接、待确认知识 | 决定 Agent 长什么样 |
| 协议 | MCP | Shell 与记忆层之间唯一的通道 | —— |

三条边界是硬的：

1. **Shell 不复制 Repotory 的检索逻辑。** 上下文从 `repotory_context` 来，不自己排 FTS、不自己算 embedding、不自建向量库。
2. **Shell 不直连 Repotory 的数据库。** 只走 MCP 工具。数据库是 Repotory 的私有实现，Shell 没有理由知道它的存在。
3. **Shell 不做完整 IDE。** 路由是 Repotory 的（`repotory_notes_read` / `repotory_notes_update` 等），Shell 只消费。

## 第一版产品边界

Phase 0 定下的五项，做到就算第一版：

| 能力 | 含义 | 做到什么算完 |
|---|---|---|
| 聊天 | 与本地模型就当前项目对话，流式输出 | 选一个仓库 + 一段提问 → 有回答 |
| 读项目 | 附加仓库路径、最近 git diff、Repotory 检索到的上下文 | 附加项可勾选、可预览 |
| 执行命令 | 白名单工具执行，写操作需人工确认 | 命令完整显示、危险项默认禁止、执行有日志 |
| 记忆面板 | 当前上下文 / 待确认知识 / 跨会话结论三块 | 三块都能看到真实数据 |
| 人工确认 | 危险动作不自动执行 | 确认前能看到完整命令与影响面 |

## Phase 0 定死的决议

| 决议 | 内容 | 代价 |
|---|---|---|
| 技术栈 | Tauri v2 + Rust 后端 + TypeScript 前端 | 与 Repotory 桌面端（Wails/Go）不是同一套栈，两边共享的只有数据与协议 |
| 模型入口 | 先 Ollama；后续 llama.cpp / MLX 走插件式后端，不硬编码 | 后端注册表要先立住（`src-tauri/src/model_backends.rs`） |
| 默认模型 | `qwen3:8b`（Qwen3 8B Q4），M4 Pro 上先跑通 | 换模型要用户自己在 Ollama 里 pull |
| 记忆通道 | MCP，不自建 | Shell 依赖 Repotory 的 MCP 二进制存在 |

模型后端的注册表已经按"插件式"立住了：Ollama 是唯一实现，`llama.cpp` 与 `MLX` 声明为 `planned`，UI 调一个命令拿到全部后端状态，新增后端不需要改前端。

## 记忆层契约

Shell 要消费的 MCP 工具，都来自 Repotory 现存的 `repotory_*` 工具集（以 Repotory 主仓库为准）：

- `repotory_context` —— 项目上下文，Phase 1 的主数据源
- `repotory_handoff` —— 写会话交接，Phase 1 的出口
- `repotory_notes_search` / `repotory_notes_read` / `repotory_notes_create` / `repotory_notes_list` / `repotory_notes_update` —— 笔记读写
- `repotory_projects_list` / `repotory_projects_stats` —— 项目清单
- `repotory_scan` —— 发现本地 Git 仓库

工具名的唯一真相在 Repotory 主仓库；本仓库的 `src-tauri/src/memory_layer.rs` 里有一份副本，仅用于首屏展示，**不是**契约来源。

## 待确认（需要维护者拍板）

### 1. 路线文档里的 `repotory_pending` / `repotory_snapshot` 在 Repotory 里不存在

原路线 Phase 1 列的四个工具中，`repotory_context`、`repotory_handoff` 确实存在，但 `repotory_pending` 与 `repotory_snapshot` 在 Repotory 的 MCP 工具集里**没有对应物**：

- "待确认知识"在 Repotory 里的真实形态是 wiki 页状态 `pending / approved / rejected`（`internal/db/wiki.go`）；
- 快照（snapshot）是 Repotory 的本地备份机制（ADR-0025 L1），是内部能力，不是 MCP 工具。

**待定**：Shell 的"待确认知识"面板读哪个接口。

### 2. Phase 4 的 approve / reject 与 ADR-0015 的人类闸门冲突

Repotory 把 `ListPendingWikiPages` / `ApproveWikiPage` / `RejectWikiPage` 标为桌面专属（ADR-0015 决策 3：把内容发布进检索是人的行为，没有第二个调用者）。Shell 的记忆面板要 approve / reject，就必须越这道闸：

- **选项 A**：Shell 走本地 RPC 桥（与 JetBrains 插件同一条路）直连 Repotory 服务，绕开 MCP；
- **选项 B**：等 Repotory 侧提供一个受控的 MCP 工具，由 Repotory 自己保证只有人在场时才允许写；
- **选项 C**：Phase 4 只做只读面板（看得到待确认知识），approve 仍回 Repotory 桌面端做。

**待定**：选哪条。这条决定 Phase 4 的工作量，也决定 Shell 是不是必须依赖本地服务而不只是 MCP 进程。

### 3. MCP 客户端的"自动发现"到底自动到什么程度

路线 Phase 1 写的是"自动发现本地 Repotory MCP"。可选做法：PATH 里找 `repotory-mcp` 二进制、固定路径、还是让用户在设置里指一个。首屏目前按 PATH + 常见目录探测（`/usr/local/bin`、`/opt/homebrew/bin`、`~/.local/bin`），只做**存在性**判断，不自动拉起。

**待定**：启动时是否自动拉起 MCP 进程，以及拉起后的连接失败如何降级（路线写的是"降级为普通聊天"）。

### 4. 与 Repotory 的版本耦合

Shell 消费的 MCP 工具集会随 Repotory 演进（`repotory_*` 可能增删）。是否需要一次能力协商（Shell 问一句"你有哪些工具"，而不是假定清单），还是靠 Repotory 的语义化版本约束。

**待定**：不做能力协商的话，Shell 与 Repotory 的版本组合就是一个隐式约束，需要写进 README 与安装器。
