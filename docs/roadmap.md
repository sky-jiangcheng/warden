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

**那一圈已经闭合（2026-11-12，M2 收尾）**：「把上下文拼进模型请求 → 回答写回
`repotory_handoff`」两头都接上了——diff 注入与 handoff 写回见 Phase 2 的收尾记录。
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
  配置落在 app config 目录的 `providers.json` 里，**API Key 存同目录的 `secrets.json`、创建即 0600**
  ——配置文件里只存「有没有 Key」。**不进系统钥匙串**：早期版本 ad-hoc 签名，钥匙串 ACL 绑不住签名，
  「始终允许」每出一个新构建就得重来一次（v0.1.7 的决定，推理在 `src/providers.rs` 头注释）；
  现在发布流水线用 Developer ID 签名了，文件这条路仍然保留；
- **OpenAI 兼容客户端**（`src-tauri/src/llm.rs`）：**代码里只有一个客户端**。
  OpenRouter、OpenAI、DeepSeek、硅基流动、vLLM、LM Studio、Ollama 的 `/v1` 面
  全都说同一套话，所以加一家厂商是改配置不是改代码；
  **本机 Ollama 走同一条路**——「本地模型」和「云端模型」不是两类东西；
- **拉取模型列表**：模型清单是服务端给的，**本仓不保留副本**；
- **测试连接**：真发一次最小请求，验 key / 端点 / 模型三者都通——
  只 ping 端口证明不了任何一件；
- **流式聊天**：`tauri::ipc::Channel` 把增量推给前端，回答边写边显示；
- **代理规则**：loopback 直连，其余走 `HTTPS_PROXY` / `HTTP_PROXY`（尊重 `NO_PROXY`）。

**最后一圈已接上（2026-11-12）**：
- **git diff 注入**（`src-tauri/src/git.rs`）：带仓库范围的线程在每轮生成时现场跑一次
  `git --no-pager diff --stat` + patch 摘录，进 system 提示词——固定 argv、无 shell、
  字节上限 + 超时看门狗；干净工作区**沉默**，被截断**明说**（不静默降级同一条纪律）。
  这不是 Phase 3 的工具执行层：它共享它的纪律，不共享它的 surface。
- **回答写回 `repotory_handoff`**：assistant 回复落库成功**之后**，Warden 自己写一条
  ADR-0054 形状的 milestone 检查点（`agent=warden`、按 `path` 归属仓库、问答摘录为 summary、
  同一份 diff 快照作为「当前未完成」进 `next_steps`）。它是 Warden 自己的 capability 绑定，
  **不在模型工具面里**——模型会 append 就是双写，这条写回也一样。写回失败只进 stderr，
  **不让一次已保存的回答变成发送失败**；replay 不再补写检查点。
- 分层策略照旧（[product.md](product.md)「待确认」第 5 条）：
  `handoff` / `scan` 自动写，**长期知识（`notes_create` / `notes_update` / `notes_organize`）过一次人确认**。
  长期知识那三个的确认形态是「待确认」第 7 条，仍未定——那是 M2 之后要不要开写面的决定。

## Phase 2.5：自己认仓库（1 周）—— 已完成

copilot 从「Cinder 的一个前端」变成「本机的一个独立机器人」。

- **自己扫盘**：按用户配置的根目录找 git 仓库（`.git` 是目录才算，worktree 的 `.git` 是文件不算），
  **不经过 Cinder 的扫描器，也不读它的库**；
- **列表呈现**：仓库列成表，用户自己判断哪些是他要的；
- **收藏与忽略**：两条路——「收藏」是「这是我的活」，「忽略」是「别再问」；
  **忽略过的仓库永远不再冒泡**，否则说一次「不」还要被问第二遍；
- **新仓库冒泡**：扫到没处理过的仓库，窗口顶部弹一个气泡问「是不是你的」，
  一个一个来（五个卡片一起问是表单，不是问题）；
  **只有这一个通道**——不弹系统通知，也不带通知动作（见「工程缺口」第 2 条）；
- **后台常驻**：窗口关掉也在扫（每 5 分钟一轮），这才叫机器人而不是一张报表。

**收藏状态存 copilot 自己的 watchlist**（app config 目录的 `repos.json`），
**不写 Cinder 主库**：主库的 `projects.is_starred` 写入方是桌面端（desktop-only），
MCP 工具面里没有孪生工具——**要共享得先改主仓，那不是一个 copilot 该做的事**。

**没做**：git 元信息（分支 / 最后提交时间）——列表现在只有名字与路径。
**已做（2026-11-12，M2 收尾）**：把仓库上下文拼进模型请求——线程的仓库路径、
未提交的 diff 摘录进 system 提示，回答落库后写 milestone handoff（见 Phase 2 收尾记录）。

## Phase 3：工具执行层（2–3 周）—— 读与写两半都已接上（2026-11-13）

让 Agent 不只是聊天，还能安全执行动作。先做白名单工具：

`git status` · `git diff` · `git log` · `git branch` · `git show` · `ls / tree` · `cat / head / grep` · `npm run build` · `cargo test` · `mysql -e`（只读优先）

执行规则：

- 所有写操作必须确认；
- 所有命令必须显示完整命令；
- 所有输出必须可折叠查看；
- 危险命令默认禁止；
- 每次执行记录到本地日志。

这一阶段不要做全自动 Agent，先做 Human-in-the-loop。

**当前进度：只读那一半已落地**（`src-tauri/src/exec.rs`）。白名单是八个 `warden_*` 工具——
`git status` / `diff` / `log` / `branch` / `show`、列目录、读文件、grep——它们**只读，
所以不需要确认，可以先于确认机制上线**。四条纪律：

- **固定 argv，没有 shell**：模型只能给值（路径、rev、pattern），不能给开关；
  `-` 开头、绝对路径、`..` 上跳、rev 区间或 `@{...}` 查询，一律在开跑前拒掉并说明理由；
- **只在会话的仓库里跑**：子进程 cwd 就是仓库；读文件先 canonicalize 再比包含关系，
  指向仓库外的符号链接是拒绝而不是读取；
- **有上限、可杀掉**：与 diff 注入共用同一个 `run_confined` 运行器（字节上限 + 5 秒看门狗 +
  真杀子进程），卡住的命令降级成一条工具错误，不冻住对话；
- **每次执行进本地日志**：`exec-audit.jsonl`（app config 目录、0600、JSONL），
  记时间、仓库、工具、**完整 argv**、退出码、字节数、是否截断；
- **完整命令进 UI**：`ToolEvent` 带 `command`，工具行显示的就是真跑的那条命令行。

**写/构建那一半也接上了（同日）**——确认卡闸门，四个工具：
`warden_git_add` · `warden_git_commit` · `warden_npm_build` · `warden_cargo_test`。

- **先规划、再询问、后执行**：卡片显示的就是 `plan_write` 算出来的那条命令，
  执行走同一个 planner——**卡片上的字符串和真跑的字符串是同一个来源**，
  不存在"给你看的和跑的不一样"；
- **不回答 = 拒绝**：卡片带倒计时（180 秒），超时**不执行**，不是默认同意；
  模型被告知是"没人回答"还是"用户拒绝"，两者对它的下一步计划不一样；
- **批准只作用一次**：批准记录按卡片 id 存在内存里，命令跑完即丢弃；
  没有持久化的白名单，也没有"以后都同意"；
- **写工具也在仓库里跑**：同一个 `run_confined_within`，更长的超时（300 秒）与更大的输出上限（16 KB）；
- 拒绝与批准的命中都会写进同一份 `exec-audit.jsonl`。

**「所有输出可折叠查看」已落地（同日）**：工具行变成 `<details>`，展开就是模型收到的那份原始文本——
**不是重新执行一遍取新输出**（同一条命令再跑一次可能给出不同结果，那会让面板在骗人）。
折叠态只显示首行摘要（超长截断），展开体有高度上限，所以输出再长也推不走输入框；
重渲染不会关掉用户已展开的面板。

**还没做**：任意命令（`mysql -e`、别的 shell）仍然不在面里——
只有这四个写工具和八个读工具构成的白名单。

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

如果后续要支持 llama.cpp / MLX，它们也走同一条「一个 base URL」的路，**不要为某家开分支**：
本地 provider 与云端 provider 是同一件事。`model_backends.rs` 这个注册表已经在 Phase 2 删掉
（见 [product.md](product.md) 的「Phase 0 定死的决议」）。

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
| M2 | 能和模型聊当前仓库 | 已完成（2026-11-12：聊天链路、system 提示、只读工具面、diff 注入、handoff 写回全部接通） |
| M3 | 能安全执行 Git / Shell 命令 | 进行中（读八件 + 写四件都已落地，写侧过确认卡；审计日志、完整命令显示、可折叠输出面板就位；任意 shell 未做） |
| M4 | 能看到并管理跨会话记忆 | 未开始 |
| M5 | 能一键安装、一键初始化 | 未开始 |
| M6 | 能作为独立产品分发 | 未开始 |

## 工程缺口（不进里程碑，2026-11-11 评估）

下面三条在现有 Phase 里没有归属，但会挡「可发布」。销项以验收判据为准，不加新流程。

### 1. 前端状态机测试 —— 已销项（2026-11-12，选了 [product.md](product.md)「待确认」第 6 条的选项 ①）

- **问题**：`npm run build` 只有 `tsc`，CI 三道门里没有前端测试。
- **影响**：`src/chat.ts` 的 revision 丢弃、并发、IME 防抖、失败保留草稿只能手测，
  改一次聊天流就可能回归。
- **验收**：按 [product.md](product.md)「待确认」第 6 条选型落地，并进 CI。
- **销项**：四个决策抽成纯函数（`src/chat-state.ts`），`chat.ts` 直接调用——测的是生产路径；
  vitest 用例覆盖验收判据的四条路径（`src/chat-state.test.ts`，`npm run test:fe`），
  CI 新增 `frontend-test` job。DOM 绑定与 `Channel` 传输刻意不测，见第 6 条的理由。

### 2. 系统通知：文档承诺、代码没有 —— 已销项（2026-11-12，选了「删」这一侧）

- **问题**：Phase 2.5 与 README 曾承诺「窗口不在前台时改走系统通知」（2026-11-11 已改正），
  但 `builder()`（`src-tauri/src/lib.rs`）只注册了 opener 与 global-shortcut；
  `tauri-plugin-notification` 留在 `Cargo.toml`，却没有 `.plugin(...)`、没有 capability、代码零调用。
  扫描载荷的测试反而断言「没有 announcement 通道」。
- **影响**：依赖白挂着（包体与权限审计面）；文档承诺了一条不存在的唤回路径。
- **验收**：二选一——删掉依赖与理由注释，或补齐注册 / 权限 / 测试并恢复文档描述；
  文档与代码同一提交。
- **销项**：文档与测试都站在「不发系统通知」这一边（冒泡是唯一通道），所以删依赖这一侧
  才是与产品决定一致的修法。`Cargo.toml` 里的 `tauri-plugin-notification` 已连同它的
  理由注释一起删除，替换为「为什么没有它」的注释；`Cargo.lock` 随之变干净。
  将来若真要系统通知：注册、capability、测试、文档恢复，四条同一提交再做。

### 3. macOS 也进 CI —— 已销项（2026-11-12）

- **问题**：CI 的 Rust 三道门跑在 `ubuntu-latest`；托盘（`muda` 主线程）、Accessory 策略、
  template 图标、`tauri build` 打包都只在 macOS 上有意义。
- **影响**：发布前只能手测；托盘是否就位只能靠 `tray.installed` 自证。
- **验收**：CI 增加 macOS job（fmt + clippy + test + `tauri build --debug` 冒烟），
  失败能定位到是编译、测试还是打包；不要求签名。
- **销项**：`ci.yml` 新增 `rust-macos` job（`macos-14`）：fmt / clippy / test 与 Ubuntu job
  同一组命令，再加一步 `tauri build --debug --bundles app` 打包冒烟——四步各管一件事，红了就知道是
  编译、测试还是打包。打包只出 `.app`，不出 dmg：`hdiutil` 在 runner 上要磁盘镜像权限，
  时好时坏，这种抖动会让门禁不可信；而编译、图标阶梯、info.plist 对两者是同一套输入。
  不传签名 secret，Tauri 落回 ad-hoc 签名，足够证明打包图能走通；
  签名、公证与 dmg 仍只在 tag 触发的 release 工作流里。

## 路线取舍

不要一开始就把 copilot 做成"完整 IDE"。先让它成为一个好用的本地助手：打开就能聊项目、能看到记忆、能安全执行命令、能跨会话继续。等这条路径跑顺了，再考虑接 Claude Code、Cursor、VS Code、JetBrains，或者把它做成更通用的本地 Agent 平台。

**托盘已经落地**（ADR-0047 决策⑤）：它是「关掉工作台还有助手在」那条路的载体。
托盘图标是 copilot 的唤回入口——点它把窗口叫回来，右键是菜单（Show / Quit）。
**关窗即隐藏、托盘 Quit 才退出**：`on_window_event` 对 `CloseRequested` 做 `prevent_close` + 隐藏，
保住两个 webview 与正在跑的生成任务——红按钮不该掐断一轮回答（与 [product.md](product.md)「常驻形态」一致）。
**⌥ Space 全局快捷键已注册**（`src-tauri/src/tray.rs` 的 `SHORTCUT`），**伴侣窗常驻可见也已落地**；
**形态是否再调整仍由使用者反馈决定**。
