# 总体路线

> 来源：2026-10-28 的《总体路线》讨论稿。本页把它落成仓库里的可执行版本。
> 定位与边界见 [product.md](product.md)；该页同时记录了路线与 Cinder 现状不一致之处。

## 一张图

```
Cinder MCP 记忆层          ↑ copilot（Agent，Tauri）           ↑ 本地模型（Ollama / llama.cpp / MLX）
                          ↑ 工具层（Git / Shell / 文件索引 / 数据库 CLI）
                          ↑ 主流客户端（Claude Code / Cursor / VS Code / JetBrains）
```

记忆层通过 MCP 开放；copilot 本身只做"本地模型入口 + 命令执行 + 记忆面板"。
这里的 **copilot 指本产品**（Warden），**不是 GitHub Copilot**。

**它与 Cinder Desktop 是两个产品**：桌面端是**工作台**（浏览、编辑、审批、治理），
copilot 是**助手**（读、聊、执行）。两者共享记忆层与协议，不共享界面，也不共享生命周期。

## Phase 0：仓库与定位（1 周）—— 已完成

把 copilot 从 Cinder 里明确分出来，做成**自己的产品**，但保持强绑定。

- 仓库 `warden`（原名 `repotory-agent-shell` → `repotory-agent`，2026-11-04 改名；2026-11-05 随品牌改成 `sky-jiangcheng/warden`）；
- 明确定位：**独立 copilot**，默认接 Cinder MCP；
- 技术栈定死：Tauri v2 + Rust 后端 + TS 前端（**选 Tauri 的硬理由是它自带托盘**）；
- 模型入口先选 Ollama，后续可加 llama.cpp / MLX；
- 默认模型选 Qwen3 8B Q4，M4 Pro 上先跑通；
- 定义第一版产品边界：聊天、读项目、执行命令、记忆面板、人工确认。

**当前进度**：已完成。仓库与骨架已落地，定位/边界/决议写在 [product.md](product.md)，
首屏展示记忆层、模型运行时与**唤回入口（托盘）**的探测结果。
CI 门禁（docs mirror + 前端 build + `cargo fmt` / `clippy` / `test`）、品牌图标、
托盘（ADR-0047 决策⑤）都已就位。

## Phase 1：MCP 记忆层打通（1–2 周）—— 数据链路已完成

让 copilot 能稳定消费 Cinder 的记忆能力。

- 确认这些 MCP 工具稳定可用：`repotory_context`、`repotory_handoff`、`repotory_notes_search`、`repotory_projects_list`、`repotory_scan`（原路线中的 `repotory_pending` / `repotory_snapshot` 在 Cinder 里不存在，见 [product.md 待确认 1](product.md#1-repotory_pending-与-repotory_snapshot-在-cinder-里不存在已澄清)）；
- **工具清单动态拉取**：用 MCP 的 `tools/list` 取代那份硬编码副本（它已经漂了，见 [product.md 记忆层契约](product.md#记忆层契约)）；
- Agent 侧实现 MCP Client（**照抄两个 IDE 插件的路子**：拉起 `repotory-mcp` 子进程走 stdio）：
  - 自动发现本地 Cinder MCP；
  - 启动时检查连接状态；
  - 失败时降级为普通聊天（**首屏要说清「没有 `repotory-mcp` 就没有记忆」，不能假装**）；
- 最小验证：
  - 打开某个仓库；
  - Shell 调用 `repotory_context`；
  - 把上下文拼进模型请求；
  - 模型回答后写入 `repotory_handoff`。

这一阶段不追求 UI，先保证数据链路通。

**当前进度**：**数据链路已完成**。

- **Rust MCP stdio 客户端**（`src-tauri/src/mcp_client.rs`）：`initialize` → `notifications/initialized` → `tools/list` / `tools/call`，
  一次握手然后**长驻**（server 一直开着同一个 SQLite 库，每次调用重开等于重开一次库）；
- **工具清单动态拉取**：首屏显示的就是服务端刚报上来的那份（本机实测 **18 个工具**），
  本仓**不保留任何副本**——`memory_layer.rs` 里有一条自检测试盯着这件事；
- **首屏三态**：未安装 / 装了但连不上 / 已连接 + N 个工具，三者不混；
- **端到端测试**：真管道 + fake MCP server，验握手、工具清单、调用往返、服务端错误、以及**进程真被杀掉**。

**还没做的那一圈**：「把上下文拼进模型请求 → 回答写回 `repotory_handoff`」。
聊天界面 Phase 2 已就位，所以这条现在是 **M2 的活**，不再是「等 Phase 2」。
「失败时降级为普通聊天」的策略**已定**（见 [product.md](product.md) 的「待确认」第 3 条）：
**聊天继续 + 显式横幅说明**——记忆层连不上时聊天照常，但界面必须明说「这次没有记忆」，
**上下文装配失败绝不静默**。

## Phase 2：最小聊天 Shell（1–2 周）—— 已完成

一个能用的 Agent 入口。

- 前端做一个简单聊天界面；
- Rust 后端代理模型 API；
- 支持流式输出；
- **模型 provider 可配置**：任何符合 OpenAI 兼容标准的 provider 都能接；
- 支持附加当前仓库路径；
- 支持附加最近 git diff；
- 支持附加 Cinder 检索到的上下文；
- 所有模型请求走 Rust，不直接暴露端口。

目标：打开 Shell，选一个仓库，就能和模型聊当前项目。

**当前进度**：**聊天链路已完成，模型入口已放开**。

- **provider 配置层**（`src-tauri/src/providers.rs`）：provider 就是「一个 base URL」。
  配置落在 app config 目录的 JSON 里，**API Key 存进系统钥匙串**（macOS Keychain /
  Windows 凭据管理器 / Linux Secret Service）——配置文件里只存「有没有 Key」，
  **明文落盘的密钥就是躺在每个备份里的密钥**；
- **OpenAI 兼容客户端**（`src-tauri/src/llm.rs`）：**代码里只有一个客户端**。
  OpenRouter、OpenAI、DeepSeek、硅基流动、vLLM、LM Studio、Ollama 的 `/v1` 面
  全都说同一套话，所以加一家厂商是改配置不是改代码；
  **本机 Ollama 走同一条路**——「本地模型」和「云端模型」不是两类东西；
- **拉取模型列表**：模型清单是服务端给的，**本仓不保留副本**；
- **测试连接**：真发一次最小请求，验 key / 端点 / 模型三者都通——
  只 ping 端口证明不了任何一件；
- **流式聊天**：`tauri::ipc::Channel` 把增量推给前端，回答边写边显示；
- **代理规则**：loopback 直连，其余走 `HTTPS_PROXY` / `HTTP_PROXY`（尊重 `NO_PROXY`）。

**还没做的那一圈**：「git diff 注入」与「回答写回
`repotory_handoff`」还没接上；仓库路径已经进了 system 提示词，只读工具面
（`repotory_metrics` 等八个）也已在对话里接通——「当前仓库」这个概念 Phase 2.5 已经做出来了，
所以剩下这两件仍是**可以直接接线**的事（不再是前置阻塞）。
写回侧的分层策略**已定**（见 [product.md](product.md) 的「待确认」第 5 条）：
`handoff` / `scan` 自动写，**长期知识（`notes_create` / `notes_update` / `notes_organize`）过一次人确认**。

## Phase 2.5：自己认仓库（1 周）—— 已完成

copilot 从「Cinder 的一个前端」变成「本机的一个独立机器人」。

- **自己扫盘**：按用户配置的根目录找 git 仓库（`.git` 是目录才算，worktree 的 `.git` 是文件不算），
  **不经过 Cinder 的扫描器，也不读它的库**；
- **列表呈现**：仓库列成表，用户自己判断哪些是他要的；
- **收藏与忽略**：两条路——「收藏」是「这是我的活」，「忽略」是「别再问」；
  **忽略过的仓库永远不再冒泡**，否则说一次「不」还要被问第二遍；
- **新仓库冒泡**：扫到没处理过的仓库，窗口顶部弹一个气泡问「是不是你的」，
  一个一个来（五个卡片一起问是表单，不是问题）；
  窗口不在前台时改走系统通知，**点通知把窗口叫回来**；
- **后台常驻**：窗口关掉也在扫（每 5 分钟一轮），这才叫机器人而不是一张报表。

**收藏状态存 copilot 自己的 watchlist**（app config 目录的 `repos.json`），
**不写 Cinder 主库**：主库的 `projects.is_starred` 写入方是桌面端（desktop-only），
MCP 工具面里没有孪生工具——**要共享得先改主仓，那不是一个 copilot 该做的事**。

**没做**：把仓库上下文拼进模型请求（聊天时带上哪个仓库）、git 元信息（分支 / 最后提交时间）。
列表现在只有名字与路径。**「当前仓库」已就位，剩下的是接线不是造概念。**

## Phase 3：工具执行层（2–3 周）

让 Agent 不只是聊天，还能安全执行动作。先做白名单工具：

`git status` · `git diff` · `git log` · `git branch` · `git show` · `ls / tree` · `cat / head / grep` · `npm run build` · `cargo test` · `mysql -e`（只读优先）

执行规则：

- 所有写操作必须确认；
- 所有命令必须显示完整命令；
- 所有输出必须可折叠查看；
- 危险命令默认禁止；
- 每次执行记录到本地日志。

这一阶段不要做全自动 Agent，先做 Human-in-the-loop。

## Phase 4：记忆面板（1–2 周）

让 Cinder 的价值在 UI 上可见。三个面板：

- **当前上下文**：当前仓库、分支、最近 diff、已注入记忆；
- **已批准知识**：`repotory_wiki_search` 的只读视图（**待批准的页在 copilot 里看不见——那是工作台的事**，见 [product.md 待确认 2](product.md#2-phase-4-的-approve--reject-与-adr-0015-的人类闸门冲突已解决选项-c)）；
- **跨会话记忆**：上次会话的结论、handoff 摘要、可继续任务。

这一阶段是 copilot 和纯聊天工具拉开差距的关键。**面板是只读的**——审批永远在 Cinder Desktop 做。

## Phase 5：本地模型管理（1–2 周）

让 M4 Pro 用户不用折腾也能跑。

- 检测本地是否安装 Ollama；
- 未安装时给出安装指引；
- 自动拉取默认模型；
- 支持切换模型；
- 显示模型状态、延迟、显存/内存占用；
- 支持离线模式提示。

如果后续要支持 llama.cpp / MLX，做成插件式模型后端，不要硬编码。首屏已经把后端注册表立成这个形状（见 `src-tauri/src/model_backends.rs`）。

## Phase 6：分发与上架（1–2 周）

让 copilot 能真正被用起来。

- macOS dmg 打包；
- 可选 Homebrew cask；
- App Store 版本默认不带模型，引导用户安装 Ollama；
- 提供 `repotory-agent init` 一键初始化；
- README 里写清楚：copilot 是入口，Cinder 是记忆层，MCP 是开放协议；
- **写清版本耦合**：哪个 Agent 版本配哪个**记忆层契约版本**（契约版本是主仓 `internal/version/core.go` 的 `CoreVersion`，它答的是「工具面变了没」；不做能力协商的话这个耦合是隐式的，见 [product.md 待确认 4](product.md#4-与-cinder-的版本耦合)）。

## 里程碑

| 里程碑 | 完成标志 | 状态 |
|---|---|---|
| M1 | 能调用 Cinder MCP | 已完成（Phase 1） |
| M1.5 | **能自己认仓库，并让用户判断关注谁** | **已完成（Phase 2.5）** |
| M2 | 能和模型聊当前仓库 | 进行中（Phase 2：聊天链路已通；仓库路径已进 system 提示、只读工具面已接；diff 注入与 handoff 写回未接） |
| M3 | 能安全执行 Git / Shell 命令 | 未开始 |
| M4 | 能看到并管理跨会话记忆 | 未开始 |
| M5 | 能一键安装、一键初始化 | 未开始 |
| M6 | 能作为独立产品分发 | 未开始 |

## 路线取舍

不要一开始就把 copilot 做成"完整 IDE"。先让它成为一个好用的本地助手：打开就能聊项目、能看到记忆、能安全执行命令、能跨会话继续。等这条路径跑顺了，再考虑接 Claude Code、Cursor、VS Code、JetBrains，或者把它做成更通用的本地 Agent 平台。

**托盘已经落地**（ADR-0047 决策⑤）：它是「关掉工作台还有助手在」那条路的载体。
托盘图标是 copilot 的唤回入口——点它把窗口叫回来，右键是菜单（Show / Quit）。
**关窗口仍然结束应用**：托盘是给「已经在跑」的进程用的唤回入口，
不是「窗口藏起来只留一个图标」那种常驻——那样换来的是「找不到的窗口」而不是「还在的助手」。
**唤醒交互的具体形态仍由使用者反馈决定**（全局快捷键、窗口常驻可见都还没做）。
