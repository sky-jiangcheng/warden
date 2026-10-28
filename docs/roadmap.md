# 总体路线

> 来源：2026-10-28 的《总体路线》讨论稿。本页把它落成仓库里的可执行版本。
> 定位与边界见 [product.md](product.md)；该页同时记录了路线与 Repotory 现状不一致之处。

## 一张图

```
Repotory MCP 记忆层        ↑ 轻量 Agent Shell（Tauri）        ↑ 本地模型（Ollama / llama.cpp / MLX）
                          ↑ 工具层（Git / Shell / 文件索引 / 数据库 CLI）
                          ↑ 主流客户端（Claude Code / Cursor / VS Code / JetBrains）
```

Shell 不封闭：记忆层通过 MCP 开放；Shell 本身只做"本地模型入口 + 记忆面板 + 任务确认"。

## Phase 0：仓库与定位（1 周）—— 当前阶段

把 Agent Shell 从 Repotory 里明确分出来，但保持强绑定。

- 新建 `repotory-agent-shell` 仓库；
- 明确定位：本地 Agent Shell，默认接 Repotory MCP；
- 技术栈定死：Tauri v2 + Rust 后端 + TS 前端；
- 模型入口先选 Ollama，后续可加 llama.cpp / MLX；
- 默认模型选 Qwen3 8B Q4，M4 Pro 上先跑通；
- 定义第一版产品边界：聊天、读项目、执行命令、记忆面板、人工确认。

**当前进度**：仓库与骨架已落地，定位/边界/决议写在 [product.md](product.md)，首屏展示记忆层与模型运行时的探测结果。

## Phase 1：MCP 记忆层打通（1–2 周）

让 Shell 能稳定消费 Repotory 的记忆能力。

- 确认这些 MCP 工具稳定可用：`repotory_context`、`repotory_handoff`、`repotory_notes_search`、`repotory_projects_list`、`repotory_scan`（原路线中的 `repotory_pending` / `repotory_snapshot` 在 Repotory 里不存在，见 [product.md 待确认 1](product.md#1-路线文档里的-repotory_pending--repotory_snapshot-在-repotory-里不存在)）；
- Shell 侧实现 MCP Client：
  - 自动发现本地 Repotory MCP；
  - 启动时检查连接状态；
  - 失败时降级为普通聊天；
- 最小验证：
  - 打开某个仓库；
  - Shell 调用 `repotory_context`；
  - 把上下文拼进模型请求；
  - 模型回答后写入 `repotory_handoff`。

这一阶段不追求 UI，先保证数据链路通。

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
- **Pending 知识**：Repotory 待确认的知识，支持 approve / reject；
- **跨会话记忆**：上次会话的结论、handoff 摘要、可继续任务。

这一阶段是 Shell 和纯聊天工具拉开差距的关键。approve / reject 与 ADR-0015 人类闸门的关系见 [product.md 待确认 2](product.md#2-phase-4-的-approve--reject-与-adr-0015-的人类闸门冲突)。

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

让 Shell 能真正被用起来。

- macOS dmg 打包；
- 可选 Homebrew cask；
- App Store 版本默认不带模型，引导用户安装 Ollama；
- 提供 `repotory-agent init` 一键初始化；
- 提供 VS Code / JetBrains 插件的最小适配；
- README 里写清楚：Shell 是入口，Repotory 是记忆层，MCP 是开放协议。

## 里程碑

| 里程碑 | 完成标志 |
|---|---|
| M1 | 能调用 Repotory MCP |
| M2 | 能和本地模型聊当前仓库 |
| M3 | 能安全执行 Git / Shell 命令 |
| M4 | 能看到并管理跨会话记忆 |
| M5 | 能一键安装、一键初始化 |
| M6 | 能作为独立产品分发 |

## 路线取舍

不要一开始就把 Shell 做成"完整 IDE"。先让它成为 Repotory 的最佳本地入口：打开就能聊项目、能看到记忆、能安全执行命令、能跨会话继续。等这条路径跑顺了，再考虑接 Claude Code、Cursor、VS Code、JetBrains，或者把 Shell 做成更通用的本地 Agent 平台。
