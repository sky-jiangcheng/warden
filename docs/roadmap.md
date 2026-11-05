# 总体路线

> 来源：2026-10-28 的《总体路线》讨论稿。本页把它落成仓库里的可执行版本。
> 定位与边界见 [product.md](product.md)；该页同时记录了路线与 Repotory 现状不一致之处。

## 一张图

```
Repotory MCP 记忆层        ↑ copilot（Agent，Tauri）           ↑ 本地模型（Ollama / llama.cpp / MLX）
                          ↑ 工具层（Git / Shell / 文件索引 / 数据库 CLI）
                          ↑ 主流客户端（Claude Code / Cursor / VS Code / JetBrains）
```

记忆层通过 MCP 开放；copilot 本身只做"本地模型入口 + 命令执行 + 记忆面板"。

**它与 Repotory 桌面端是两个产品**：桌面端是**工作台**（浏览、编辑、审批、治理），
copilot 是**助手**（读、聊、执行）。两者共享记忆层与协议，不共享界面，也不共享生命周期。

## Phase 0：仓库与定位（1 周）—— 已完成

把 copilot 从 Repotory 里明确分出来，做成**自己的产品**，但保持强绑定。

- 仓库 `repotory-agent`（原名 `repotory-agent-shell`，2026-11-04 改名）；
- 明确定位：**独立 copilot**，默认接 Repotory MCP；
- 技术栈定死：Tauri v2 + Rust 后端 + TS 前端（**选 Tauri 的硬理由是它自带托盘**）；
- 模型入口先选 Ollama，后续可加 llama.cpp / MLX；
- 默认模型选 Qwen3 8B Q4，M4 Pro 上先跑通；
- 定义第一版产品边界：聊天、读项目、执行命令、记忆面板、人工确认。

**当前进度**：已完成。仓库与骨架已落地，定位/边界/决议写在 [product.md](product.md)，
首屏展示记忆层、模型运行时与**唤回入口（托盘）**的探测结果。
CI 门禁（docs mirror + 前端 build + `cargo fmt` / `clippy` / `test`）、品牌图标、
托盘（ADR-0047 决策⑤）都已就位。

## Phase 1：MCP 记忆层打通（1–2 周）—— 数据链路已完成

让 copilot 能稳定消费 Repotory 的记忆能力。

- 确认这些 MCP 工具稳定可用：`repotory_context`、`repotory_handoff`、`repotory_notes_search`、`repotory_projects_list`、`repotory_scan`（原路线中的 `repotory_pending` / `repotory_snapshot` 在 Repotory 里不存在，见 [product.md 待确认 1](product.md#1-repotory_pending-与-repotory_snapshot-在-repotory-里不存在已澄清)）；
- **工具清单动态拉取**：用 MCP 的 `tools/list` 取代那份硬编码副本（它已经漂了，见 [product.md 记忆层契约](product.md#记忆层契约)）；
- Agent 侧实现 MCP Client（**照抄两个 IDE 插件的路子**：拉起 `repotory-mcp` 子进程走 stdio）：
  - 自动发现本地 Repotory MCP；
  - 启动时检查连接状态；
  - 失败时降级为普通聊天（**首屏要说清「没装 Repotory 就没有记忆」，不能假装**）；
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

**还没做的那一圈**：「把上下文拼进模型请求 → 回答写回 `repotory_handoff`」需要聊天界面，
那是 Phase 2 的东西——**没有模型请求就没什么可拼的**。
同理，「失败时降级为普通聊天」也要等 Phase 2：现在失败时首屏直说「连不上」并给一个重试按钮，
而不是假装记忆还在。

## Phase 2：最小聊天 Shell（1–2 周）

一个能用的本地 Agent 入口。

- 前端做一个简单聊天界面；
- Rust 后端代理本地模型 API；
- 支持流式输出；
- 支持附加当前仓库路径；
- 支持附加最近 git diff；
- 支持附加 Repotory 检索到的上下文；
- 所有模型请求走 Rust，不直接暴露端口。

目标：打开 Shell，选一个仓库，就能和本地模型聊当前项目。

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

让 Repotory 的价值在 UI 上可见。三个面板：

- **当前上下文**：当前仓库、分支、最近 diff、已注入记忆；
- **已批准知识**：`repotory_wiki_search` 的只读视图（**待批准的页在 copilot 里看不见——那是工作台的事**，见 [product.md 待确认 2](product.md#2-phase-4-的-approve--reject-与-adr-0015-人类闸门冲突已解决选项-c)）；
- **跨会话记忆**：上次会话的结论、handoff 摘要、可继续任务。

这一阶段是 copilot 和纯聊天工具拉开差距的关键。**面板是只读的**——审批永远在 Repotory 桌面端做。

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
- README 里写清楚：copilot 是入口，Repotory 是记忆层，MCP 是开放协议；
- **写清版本耦合**：哪个 copilot 版本配哪个 Repotory 版本（不做能力协商的话这是隐式约束，见 [product.md 待确认 4](product.md#4-与-repotory-的版本耦合)）。

## 里程碑

| 里程碑 | 完成标志 | 状态 |
|---|---|---|
| M1 | 能调用 Repotory MCP | 已完成（Phase 1） |
| M2 | 能和本地模型聊当前仓库 | 下一个（Phase 2） |
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
