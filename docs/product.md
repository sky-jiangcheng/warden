# 定位与边界

> 本页定死 Phase 0 的三件事：这个 copilot 是什么、第一版做到哪、以及哪些决定要留给维护者拍板。
> 路线（Phase 与里程碑）在 [roadmap.md](roadmap.md)。

## 一句话定位

**Repotory Agent 是一个独立产品：一个跑在本机的 copilot，不是又一个 Agent 平台，也不是 Repotory 桌面端的一个界面。** 它把"跟本地模型聊当前项目"这件事做成一个窗口，把"记得什么"交给 Repotory。

## 三个角色，三条边界

| 角色 | 归属 | 负责 | 不负责 |
|---|---|---|---|
| copilot | Agent（本仓库） | 本地模型入口、命令执行、记忆面板 | 检索排序、项目知识存储、**审批** |
| 记忆 | [Repotory](https://github.com/sky-jiangcheng/repotory) | 项目上下文、会话交接、已批准的知识页 | 决定 copilot 长什么样 |
| 协议 | MCP | Agent 与记忆层之间唯一的通道 | —— |

三条边界是硬的：

1. **Agent 不复制 Repotory 的检索逻辑。** 上下文从 `repotory_context` 来，不自己排 FTS、不自己算 embedding、不自建向量库。
2. **Agent 不直连 Repotory 的数据库。** 只走 MCP 工具。数据库是 Repotory 的私有实现，Agent 没有理由知道它的存在。
3. **Agent 不做完整 IDE。** 路由是 Repotory 的（`repotory_notes_read` / `repotory_notes_update` 等），Agent 只消费。

## 与 Repotory 桌面端的关系：两个产品，各担一件事

**不是主从，不是同一产品的两个前端。**

| | Repotory 桌面端 | Agent（copilot） |
|---|---|---|
| 定位 | **工作台** | **助手** |
| 干什么 | 浏览、编辑、**审批**、治理 | 读、聊、执行 |
| 记忆通道 | 直接用 `internal/service` | **只走 MCP** |
| 审批（approve / reject） | **有**（人在场） | **没有**——MCP 工具面里就没有这两个工具 |
| 关掉它 | 工作台没了 | copilot 照常跑 |
| 界面 | 三栏外壳 + 小机器人浮层 | 自己的窗口 + 托盘 |

**审批权留在人手里，而且不是靠约定维持的。** Repotory 的 MCP 工具面在知识页上只注册了两个**纯读**工具（`repotory_wiki_search` / `repotory_wiki_read`），
写侧的闸门是"把内容发布进检索是人的行为"（ADR-0015 决策③）。
**Agent 走 MCP，就自动站在闸门外——它想越都越不过去。**

**Agent 需要 `repotory-mcp` 二进制存在才能拿到记忆。**
**没装 Repotory 就没有记忆**——这一点必须在首屏说清楚，不能假装。

## 第一版产品边界

Phase 0 定下的五项，做到就算第一版：

| 能力 | 含义 | 做到什么算完 |
|---|---|---|
| 聊天 | 与本地模型就当前项目对话，流式输出 | 选一个仓库 + 一段提问 → 有回答 |
| 读项目 | 附加仓库路径、最近 git diff、Repotory 检索到的上下文 | 附加项可勾选、可预览 |
| 执行命令 | 白名单工具执行，写操作需人工确认 | 命令完整显示、危险项默认禁止、执行有日志 |
| 记忆面板 | 当前上下文 / 已批准知识 / 跨会话结论三块 | 三块都能看到真实数据 |
| 人工确认 | 危险动作不自动执行 | 确认前能看到完整命令与影响面 |

## Phase 0 定死的决议

| 决议 | 内容 | 代价 |
|---|---|---|
| 技术栈 | Tauri v2 + Rust 后端 + TypeScript 前端 | 与 Repotory 桌面端（Wails/Go）不是同一套栈，两边共享的只有数据与协议 |
| **选 Tauri 的理由** | **它自带托盘**（菜单栏图标 + 点击唤起）——Wails v2 没有托盘（`pkg/menu` 的 `TrayMenu` 是死类型，外部 `systray` 要 cgo）。**托盘已落地**（`src-tauri/src/tray.rs`）：左键唤回窗口，右键是 Show / Quit 菜单 | **Tauri 用系统 WebView，不是 Swift/AppKit 原生控件**——它是"独立进程 + 真托盘 + 真分发渠道"，**不是"原生 UI 框架"** |
| 模型入口 | 先 Ollama；后续 llama.cpp / MLX 走插件式后端，不硬编码 | 后端注册表要先立住（`src-tauri/src/model_backends.rs`） |
| 默认模型 | `qwen3:8b`（Qwen3 8B Q4），M4 Pro 上先跑通 | 换模型要用户自己在 Ollama 里 pull |
| 记忆通道 | MCP，不自建 | Agent 依赖 Repotory 的 MCP 二进制存在 |

模型后端的注册表已经按"插件式"立住了：Ollama 是唯一实现，`llama.cpp` 与 `MLX` 声明为 `planned`，UI 调一个命令拿到全部后端状态，新增后端不需要改前端。

**「看用户喜好」的部分明确交给用户**：模型后端、界面形态都不由本仓决定。

**唤回入口已经落地，但「唤回交互」的整体形态仍然待定**：托盘图标是当前的入口（左键唤回窗口、右键菜单），
首屏有一张「唤回入口」卡片实时显示托盘是否就位——macOS 不允许外部进程查询别人的菜单栏状态项，
所以「托盘到底在不在」只能由应用自己回答（`tray.installed`），托盘装不上时日志里有一行 `[tray] ...`。
**全局快捷键、窗口常驻可见都还没做**，等使用者反馈再定。

## 记忆层契约

Agent 要消费的 MCP 工具，都来自 Repotory 现存的 `repotory_*` 工具集（以 Repotory 主仓库为准）：

- `repotory_context` —— 项目上下文，Phase 1 的主数据源
- `repotory_handoff` —— 写会话交接，Phase 1 的出口
- `repotory_notes_search` / `repotory_notes_read` / `repotory_notes_create` / `repotory_notes_list` / `repotory_notes_update` / `repotory_notes_organize` —— 笔记读写
- `repotory_projects_list` / `repotory_projects_stats` —— 项目清单
- `repotory_wiki_search` / `repotory_wiki_read` —— **已批准**的知识页（纯读）
- `repotory_scan` —— 发现本地 Git 仓库
- `repotory_ask` —— 带出处的证据问答（ADR-0033）
- `repotory_agent_score` / `repotory_integrity` —— 自省

工具名的唯一真相在 Repotory 主仓库，**本仓库不保留那份清单的运行时副本**。
Phase 1 起那份副本已经删掉：工具清单在连接时用 MCP 的 `tools/list` 动态拉取
（`src-tauri/src/mcp_client.rs`），首屏显示的就是服务端刚报上来的那份。
**Phase 0 那份副本只列了 7 个名字，而服务端注册了 18 个**——
少列的里头包含 `repotory_wiki_search` / `repotory_wiki_read`（读已批准知识的唯一入口）
和 `repotory_ask`（带出处的证据问答）。
**一份会漂的清单比没有清单更坏，因为它让人以为那是真的**——
现在 `memory_layer.rs` 里有一条自检测试盯着别把工具名写回源码。

> 上面这份列表是**给人看的说明，不是运行时契约**：本机实测服务端报上来 **18 个**工具，
> 与上面列出的名字一致。**运行时永远以服务端报上来的为准**——
> Repotory 加了工具，copilot 这边不用改代码就能看见。

## 待确认（需要维护者拍板）

### 1. ~~`repotory_pending` 与 `repotory_snapshot` 在 Repotory 里不存在~~（已澄清）

原路线 Phase 1 列的四个工具中，`repotory_context`、`repotory_handoff` 确实存在，但 `repotory_pending` 与 `repotory_snapshot` 在 Repotory 的 MCP 工具集里**没有对应物**：

- "待确认知识"在 Repotory 里的真实形态是 wiki 页状态 `pending / approved / rejected`（`internal/db/wiki.go`）；
- 快照（snapshot）是 Repotory 的本地备份机制（ADR-0025 L1），是内部能力，不是 MCP 工具。

**结论**：**待确认知识面板 = `repotory_wiki_search` 的只读视图。**
MCP 面上没有"列出待批准页"的工具——**因为「谁能看到未批准的内容」本身就是闸门的一部分**。
面板因此只能显示已批准的页，**待批准的页在 copilot 里看不见**（这是对的：那是工作台的事）。

### 2. ~~Phase 4 的 approve / reject 与 ADR-0015 的人类闸门冲突~~（已解决：选项 C）

原问题：Phase 4 的记忆面板想做 approve / reject，但 ADR-0015 把
`ApproveWikiPage` / `RejectWikiPage` 标为桌面专属（"把内容发布进检索是人的行为，没有第二个调用者"）。

**现在不需要拍板了——MCP 工具面本来就只有纯读的知识页工具**
（`cmd/mcp/tools_wiki.go` 只注册 `wiki_search` / `wiki_read`，注释写明闸门在写侧）。
**走 MCP 就自动站在闸门外**，不需要额外的机制。

**结论：选项 C，且它不是妥协，是分工。**
**审批永远在 Repotory 桌面端（工作台）做**——那里有人在场。
copilot 的记忆面板是**只读**的。

### 3. ~~MCP 客户端的"自动发现"到底自动到什么程度~~（已定：自动拉起）

原问题：Phase 1 写的是"自动发现本地 Repotory MCP"。可选做法：PATH 里找 `repotory-mcp` 二进制、固定路径、还是让用户在设置里指一个。

**现在这样**：按 PATH + 常见目录探测（`/usr/local/bin`、`/opt/homebrew/bin`、`~/.local/bin`），
找到就**自动拉起**子进程走 stdio——**和两个 IDE 插件一样**
（VS Code / JetBrains 都是自己拉起 `repotory-mcp`，那条路已经跑通了）。
拉起后做一次握手（`initialize` → `notifications/initialized`），
然后用 `tools/list` 拿工具清单，**长驻**会话（server 一直开着同一个 SQLite 库，
每次调用重开等于每次重开库）。

**为什么自动拉起是对的**：不拉起就没有记忆，copilot 的存在意义就没了；
而"只探测不拉起"会让首屏显示"已安装"——**那是在承诺一个没有验证过的东西**。
现在首屏分三态：未安装 / 装了但连不上 / 已连接 + 服务端给的工具清单。

**还开着的一半**：「连不上时降级为普通聊天」要等 Phase 2——
现在失败时首屏直说「连不上」并给一个重试按钮，**不假装记忆还在**。

### 4. 与 Repotory 的版本耦合

Agent 消费的 MCP 工具集会随 Repotory 演进（`repotory_*` 可能增删）。是否需要一次能力协商
（Agent 问一句"你有哪些工具"，而不是假定清单），还是靠 Repotory 的语义化版本约束。

**待定**：不做能力协商的话，Agent 与 Repotory 的版本组合就是一个隐式约束，
**需要写进 README 与安装器**（不能只在脑子里）。
**已定的部分**：工具清单**动态拉取**（见上节），所以新增工具不会因为清单没更新而不可见——
**能力协商解决的是"工具被删了/改名了"，不是"工具新增了"**。

### 5. copilot 要不要执行写操作

本仓定的是"走 MCP，只有一组写工具"（`notes_create` / `notes_update` / `notes_organize` / `handoff`）。
但**"要不要让 copilot 写笔记"是产品决定，不是技术决定**。

**待定**：写操作要不要也要求人工确认（Phase 0 的「人工确认」能力目前只覆盖命令执行）。
