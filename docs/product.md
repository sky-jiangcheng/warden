# 定位与边界

> 本页定死 Phase 0 的三件事：这个助手是什么、第一版做到哪、以及哪些决定要留给维护者拍板。
> 路线（Phase 与里程碑）在 [roadmap.md](roadmap.md)。

## 一句话定位

**Warden 是一个独立产品：一个跑在本机的 copilot（**不是** GitHub Copilot），不是又一个 Agent 平台，也不是 Cinder Desktop 的一个界面。** 它把"跟本地模型聊当前项目"这件事做成一个窗口，把"记得什么"交给 Cinder。

**它与 Cinder Desktop 是两个产品，各担一件事**：Cinder Desktop 是**工作台**（浏览、编辑、**审批**、治理），Warden 是**助手**（读、聊、执行）。两者共享同一份本地记忆、同一套协议，**审批永远在 Cinder Desktop 那边**。

## 三个角色，三条边界

| 角色 | 归属 | 负责 | 不负责 |
|---|---|---|---|
| copilot | Agent（本仓库） | **本机仓库发现与用户关注清单**、本地模型入口、命令执行、记忆面板 | 检索排序、项目知识存储、**审批** |
| 记忆 | [Cinder](https://github.com/sky-jiangcheng/cinder) | 项目上下文、会话交接、已批准的知识页 | 决定 copilot 长什么样 |
| 协议 | MCP | Agent 与记忆层之间唯一的通道 | —— |

三条边界是硬的：

1. **Agent 不复制 Cinder 的检索逻辑。** 上下文从 `repotory_context` 来，不自己排 FTS、不自己算 embedding、不自建向量库。
2. **Agent 不直连 Cinder 的数据库。** 只走 MCP 工具。数据库是 Cinder 的私有实现，Agent 没有理由知道它的存在。
3. **Agent 不做完整 IDE。** 路由是 Cinder 的（`repotory_notes_read` / `repotory_notes_update` 等），Agent 只消费。
4. **Agent 自己认仓库，不借 Cinder 的手。** 扫盘是 copilot 自己的事：它按用户给的根目录找 git 仓库，
   自己列出来、自己记住用户关注了哪些。**仓库清单不进 Cinder 的库**——那台库是知识库，不是「用户正在做什么」的登记处。

## 与 Cinder Desktop 的关系：两个产品，各担一件事

**不是主从，不是同一产品的两个前端。**

| | Cinder Desktop | Agent（copilot） |
|---|---|---|
| 定位 | **工作台** | **助手** |
| 干什么 | 浏览、编辑、**审批**、治理 | 读、聊、执行 |
| 记忆通道 | 直接用 `internal/service` | **只走 MCP** |
| 审批（approve / reject） | **有**（人在场） | **没有**——MCP 工具面里就没有这两个工具 |
| 关掉它 | 工作台没了 | copilot 照常跑 |
| 仓库从哪来 | 它自己的扫描根（`scan_roots`） | **它自己的扫描根 + 自己的收藏清单** |
| 界面 | 三栏外壳 + 小机器人浮层 | 自己的窗口 + 托盘 |

**审批权留在人手里，而且不是靠约定维持的。** Cinder 的 MCP 工具面在知识页上只注册了两个**纯读**工具（`repotory_wiki_search` / `repotory_wiki_read`），
写侧的闸门是"把内容发布进检索是人的行为"（ADR-0015 决策③）。
**Agent 走 MCP，就自动站在闸门外——它想越都越不过去。**

**Agent 需要 `repotory-mcp` 二进制存在才能拿到记忆。**
**没有 `repotory-mcp` 就没有记忆**——而那个二进制**是独立分发的 release 产物**（每个平台一份），
**桌面 App 不是必需品**；这一点必须在首屏说清楚，不能假装。

## 第一版产品边界

Phase 0 定下的五项，做到就算第一版；Phase 2.5 又加了一项「认仓库」：

| 能力 | 含义 | 做到什么算完 |
|---|---|---|
| **认仓库** | **自己扫盘列出本机仓库，用户判断哪些要关注** | **新仓库冒泡提醒，收藏后进入列表** |
| 聊天 | 与本地模型就当前项目对话，流式输出 | 选一个仓库 + 一段提问 → 有回答 |
| 读项目 | 附加仓库路径、最近 git diff、Cinder 检索到的上下文 | 附加项可勾选、可预览 |
| 执行命令 | 白名单工具执行，写操作需人工确认 | 命令完整显示、危险项默认禁止、执行有日志 |
| 记忆面板 | 当前上下文 / 已批准知识 / 跨会话结论三块 | 三块都能看到真实数据 |
| 人工确认 | 危险动作不自动执行 | 确认前能看到完整命令与影响面 |

**「人工确认」覆盖两处**：白名单命令执行（Phase 3），以及 copilot 通过 MCP 写**长期知识**
（`notes_create` / `notes_update` / `notes_organize`）——见下文「待确认」第 5 条。

## Phase 0 定死的决议

| 决议 | 内容 | 代价 |
|---|---|---|
| 技术栈 | Tauri v2 + Rust 后端 + TypeScript 前端 | 与 Cinder Desktop（Wails/Go）不是同一套栈，两边共享的只有数据与协议 |
| **选 Tauri 的理由** | **它自带托盘**（菜单栏图标 + 点击唤起）——Wails v2 没有托盘（`pkg/menu` 的 `TrayMenu` 是死类型，外部 `systray` 要 cgo）。**托盘已落地**（`src-tauri/src/tray.rs`）：左键唤回窗口，右键是 Show / Quit 菜单 | **Tauri 用系统 WebView，不是 Swift/AppKit 原生控件**——它是"独立进程 + 真托盘 + 真分发渠道"，**不是"原生 UI 框架"** |
| 模型入口 | **任何符合 OpenAI 兼容标准的 provider**，不硬编码厂商 | 一套客户端通吃，本地与云端同一条路 |
| 默认 provider | 本机 Ollama（`http://127.0.0.1:11434/v1`）——不需要账号也不需要 Key | 想要别的模型得自己配一个 provider |
| 默认模型 | `qwen3:8b`（Qwen3 8B Q4），M4 Pro 上先跑通 | 换模型要用户自己在 Ollama 里 pull |
| 记忆通道 | MCP，不自建 | Agent 依赖 Cinder 的 MCP 二进制存在 |
| 仓库发现 | **copilot 自己扫盘**，用户配根目录、自己决定关注谁 | **收藏清单与 Cinder 主库各存各的**——想共享得改主仓，那不是一个 copilot 该做的事 |

**模型入口在 Phase 2 已经放开**：provider 就是「一个 base URL」，
OpenRouter / OpenAI / DeepSeek / 硅基流动 / vLLM / LM Studio / Ollama 的 `/v1` 面
全都说同一套协议，所以**代码里只有一个客户端**（`src-tauri/src/llm.rs`），
**加一家厂商是改配置不是改代码**。

原先那个「本地运行时注册表」（`model_backends.rs`，Ollama 已实现、llama.cpp 与 MLX 声明为
`planned`）**已经删掉**：它把「本地模型」和「云端模型」当成两类东西，
于是每加一家厂商都要改代码。**本机 Ollama 现在走的就是远程 provider 那条路**
（`http://127.0.0.1:11434/v1`）——「装了 Ollama」和「配了 OpenRouter」是同一件事。

**API Key 存在 app config 目录的 `secrets.json` 里，创建即 0600**（不再走系统钥匙串：
本仓是 ad-hoc 签名，钥匙串 ACL 绑不住签名，「始终允许」每出新版本就得重来一次），
provider 配置文件里只存「有没有 Key」。

**「看用户喜好」的部分明确交给用户**：模型后端、界面形态都不由本仓决定。

**唤回入口已经落地，但「唤回交互」的整体形态仍然待定**：托盘图标是当前的入口（左键唤回窗口、右键菜单），
首屏有一张「唤回入口」卡片实时显示托盘是否就位——macOS 不允许外部进程查询别人的菜单栏状态项，
所以「托盘到底在不在」只能由应用自己回答（`tray.installed`），托盘装不上时日志里有一行 `[tray] ...`。
**全局快捷键、窗口常驻可见都还没做**，等使用者反馈再定。

## 记忆层契约

Agent 要消费的 MCP 工具，都来自 Cinder 现存的 `repotory_*` 工具集（以 Cinder 主仓库为准）：

- `repotory_context` —— 项目上下文，Phase 1 的主数据源
- `repotory_handoff` —— 写会话交接，Phase 1 的出口
- `repotory_notes_search` / `repotory_notes_read` / `repotory_notes_create` / `repotory_notes_list` / `repotory_notes_update` / `repotory_notes_organize` —— 笔记读写
- `repotory_projects_list` / `repotory_projects_stats` —— 项目清单
- `repotory_wiki_search` / `repotory_wiki_read` —— **已批准**的知识页（纯读）
- `repotory_scan` —— 发现本地 Git 仓库
- `repotory_ask` —— 带出处的证据问答（ADR-0033）
- `repotory_agent_score` / `repotory_integrity` —— 自省
- `repotory_ai_audit` —— 这台机器最近把什么发给了 AI：时间、端点、模型、哪个面、字符数、有没有被本地审阅规则拦下
- `repotory_ai_review_rules` —— 本地 AI 审阅规则与入/出向开关（**只读**）
- `repotory_chat` —— **共享会话**：主题与消息跨端读写（`list` / `create` / `read` / `append` / `rename`），Warden、VS Code、JetBrains 看到的是**同一份**记录
- `repotory_metrics` —— **量化数据**：读记忆层里已存的仓库与知识计数（不扫盘、不跑 git、不刷缓存）

工具名的唯一真相在 Cinder 主仓库，**本仓库不保留那份清单的运行时副本**。
Phase 1 起那份副本已经删掉：工具清单在连接时用 MCP 的 `tools/list` 动态拉取
（`src-tauri/src/mcp_client.rs`），首屏显示的就是服务端刚报上来的那份。
**Phase 0 那份副本只列了 7 个名字，而服务端当时注册了 18 个**（今天 20 个，见下）——
少列的里头包含 `repotory_wiki_search` / `repotory_wiki_read`（读已批准知识的唯一入口）
和 `repotory_ask`（带出处的证据问答）。
**一份会漂的清单比没有清单更坏，因为它让人以为那是真的**——
现在 `memory_layer.rs` 里有一条自检测试盯着别把工具名写回源码。

> 上面这份列表是**给人看的说明，不是运行时契约**：本机实测服务端报上来 **20 个**工具，
> 与上面列出的名字一致。**运行时永远以服务端报上来的为准**——
> Cinder 加了工具，copilot 这边不用改代码就能看见。

**工具面之外，这层契约还有一个版本号**：主仓在 ADR-0048 里给记忆层单开了一个
**契约版本**（`internal/version/core.go` 的 `CoreVersion`），MCP `initialize` 响应的
`serverInfo.version` 报的就是它。**它答的是「工具面变了没」，不是「构建新不新」**——
主仓那边把版本号钉在工具清单旁边并用 CI 门禁兜住（改了工具面必须涨版本号），
所以「哪个版本配哪个版本」这个隐式约束，**在主仓那里已经变成一个可以读出来的数**。
**本仓不抄那个数**（同工具清单一条规矩），连上时读服务端的。

## 常驻形态、对话即界面与跨端同步（2026-11-06 落地）

用户给这个产品的定位是三句话：**常驻的小机器人**、**快速看到代码量这类量化数据**、**可以切主题的对话框，而且 IDE 插件跟它的聊天面板同步**。三句都已落地，各自的边界如下。

**① 常驻 = 伴侣窗 + 托盘，不是「窗口永远在场」**。伴侣窗 132×128（`index.html?surface=companion`），主窗 480×760；点伴侣开聊天。**窗里只有灯体和一句话气泡**：指标行、状态行、「对话」按钮都删了——数字归对话（见②），入口归点击 / `⌥Space` / 托盘三处，窗前那几行字只是在角色身上叠了一层界面。**只有灯体发起原生拖拽**（`window_drag`），控件一律不带 `data-tauri-drag-region`——把插图做成按钮，用户点不动还会以为程序坏了。**关窗即隐藏、托盘 Quit 才退出**：`on_window_event` 对 `CloseRequested` 做的是 `prevent_close` + 隐藏，代码里写的理由是「保住两个 webview 与它们正在跑的生成任务」——**红按钮不该把一轮正在生成的回答掐断**；Warden 又是 `repotory-mcp` 的父进程，进程退出会连带断掉那条会话通道。**Cinder Desktop 反过来已经做到关窗即退**（ADR-0047 补注），它不托管任何长驻通道。

**② 对话即界面：量化数据走对话里的工具调用，主窗不再有常驻面板**（同日落地）。模型请求现在带 `tools`——只读白名单从服务端 `tools/list` 现场过滤（`repotory_metrics` / `repotory_context` / `projects_list` / `projects_stats` / `notes_search` / `notes_read` / `wiki_search` / `wiki_read`），**schema 一字不改地用服务端的**；模型点名白名单外的工具（包括服务端 advertised 的写工具）得到的是**客户端拒绝**，永不执行；`repotory_chat` 整个不在模型工具面里——**回复落库是 Warden 自己做的，模型再会 append 就是双写**。轮数上限 5，超限后强制一轮纯文本作答。工具调用只在对话流里留一个「调用中 / 已完成」的状态行，**不写进 `repotory_chat`**——它是 UI 层的过程，不是会话产物。无 Cinder 的临时对话不带工具、不带 system 提示词，单轮契约不变。

**③ 数字还是在对话里问，不现场算，也不再挂在伴侣窗上**。要代码量、要提交数，就让模型调 `repotory_metrics`（一次性工具调用，不再有 15 秒轮询的常驻指标行）。**这条口径是刻意的**：那个工具**不扫盘、不跑 git、不刷缓存**，所以出数快、也不会因为一次提问就改变库里的东西；`totalLines` 只在缓存的语言 LOC 覆盖整个明确范围时才给数，**否则是 null——宁可显示「没有这个数」，不显示一个猜出来的数**。

**④ 跨端同步的是同一份会话，不是两份长得像的记录**。主题清单与消息各 2.5 秒轮询，数据源是记忆里那两张新表（`chat_threads` / `chat_messages`），Warden、VS Code、JetBrains 三个客户端各自长驻着一条到 `repotory-mcp` 的连接、读写同一份。**三条硬约束**：

- **只同步已完成的消息**。半截的流式输出不落库——**跨端逐 token 要把每个 token 过一遍 stdio 再落到另一个进程的 UI，换来的不是「同步」，是「经常不同步」**。所以状态条说的是「已同步 / 未同步」，**不是「正在打字」**。
- **消息不可变**。消息 id 是客户端生成的 UUID（助手回复用 `${uuid}-reply`）：同 id 同载荷重放 = 幂等成功，同 id 不同载荷 = **报错，绝不覆盖**。**三个客户端在同一条线程上并发追加，唯一守得住「谁的哪句话」的办法是不许改、只许追加**。
- **来源如实标注**。一条助手消息标成「Warden」还是「助手 · vscode」，读的是消息自己的 `source` 字段——**把插件写回的回复标成 Warden，等于替另一个客户端认领答案**。

**没有共享主题、也没有历史的那段对话是「临时对话」**：只留在当前窗口，状态条明说「临时对话 / 未同步」——**不假装它进了共享层**。记忆层连不上时聊天照常可用，这条与「待确认」第 3 条那个已定决定一致。

**⑤ 前端不直连任何端口**。所有同步都是 `invoke → Rust → MCP`；`src/` 里没有一处 `fetch` 打到某个本地端口。这不是风格偏好：**「Agent 只走 MCP，不碰别的通道」是上面那条硬边界**，共享会话没理由成为第一个例外——多一条通道就多一处「谁在写这份记忆」的答案。


## 待确认（需要维护者拍板）

### 1. ~~`repotory_pending` 与 `repotory_snapshot` 在 Cinder 里不存在~~（已澄清）

原路线 Phase 1 列的四个工具中，`repotory_context`、`repotory_handoff` 确实存在，但 `repotory_pending` 与 `repotory_snapshot` 在 Cinder 的 MCP 工具集里**没有对应物**：

- "待确认知识"在 Cinder 里的真实形态是 wiki 页状态 `pending / approved / rejected`（`internal/db/wiki.go`）；
- 快照（snapshot）是 Cinder 的本地备份机制（ADR-0025 L1），是内部能力，不是 MCP 工具。

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
**审批永远在 Cinder Desktop（工作台）做**——那里有人在场。
copilot 的记忆面板是**只读**的。

### 3. ~~MCP 客户端的"自动发现"到底自动到什么程度~~（已定：自动拉起）

原问题：Phase 1 写的是"自动发现本地 Cinder MCP"。可选做法：PATH 里找 `repotory-mcp` 二进制、固定路径、还是让用户在设置里指一个。

**现在这样**：按 PATH + 常见目录探测（`/usr/local/bin`、`/opt/homebrew/bin`、`~/.local/bin`），
找到就**自动拉起**子进程走 stdio——**和两个 IDE 插件一样**
（VS Code / JetBrains 都是自己拉起 `repotory-mcp`，那条路已经跑通了）。
拉起后做一次握手（`initialize` → `notifications/initialized`），
然后用 `tools/list` 拿工具清单，**长驻**会话（server 一直开着同一个 SQLite 库，
每次调用重开等于每次重开库）。

**为什么自动拉起是对的**：不拉起就没有记忆，copilot 的存在意义就没了；
而"只探测不拉起"会让首屏显示"已安装"——**那是在承诺一个没有验证过的东西**。
现在首屏分三态：未安装 / 装了但连不上 / 已连接 + 服务端给的工具清单。

**已定（2026-11-05）**：**聊天继续 + 显式横幅说明**——记忆层连不上时聊天照常可用，但界面必须明说「这次没有记忆」，**上下文装配失败绝不静默**（静默降级等于让模型在没有依据的情况下回答，而用户看不出它这次没有依据）。**今天已经是这个形状**：聊天与记忆层是两条独立的路（`chat_send` 不依赖 MCP 状态），记忆层只是一张独立卡片；这条决定真正约束的是**将来接上上下文注入之后**的行为——不是"能不能聊"，而是"这次有没有依据，而且说没说过"。

### 4. ~~与 Cinder 的版本耦合~~（已定：运行时自证 + 人话警告）

原问题：Agent 消费的 MCP 工具集会随 Cinder 演进（`repotory_*` 可能增删）。是否需要一次能力协商
（Agent 问一句"你有哪些工具"，而不是假定清单），还是靠 Cinder 的语义化版本约束。

**已定（2026-11-05）：运行时自证 + 人话警告，不做硬拒绝。**

① **服务端版本本来就在手边**——Cinder 的 MCP server 在 `initialize` 响应里报 `serverInfo`
（主仓 `cmd/mcp/main.go` 的 `server.NewMCPServer("repotory-mcp", version.CoreVersion)`），
而本仓的客户端**把整个响应丢掉了**（`mcp_client.rs:250-259` 拿到结果只 `?`，不接变量）。
接住它是一次改动，**不是新协议**。
② **首屏自证**：显示「**记忆层 vX.Y.Z · N 工具**」——版本与工具数都来自服务端，
**本仓不留副本**（与工具清单同一条规矩）。**那个 X.Y.Z 是契约版本**（主仓 ADR-0048 新增的
`internal/version/core.go` 的 `CoreVersion`）：**它答的是「工具面变了没」，不是「构建新不新」**——
发布版本号在 `/health` 与启动日志里，但**对「我能依赖什么」这个问题没有答案**
（界面改动天天有，工具面几个月才动一次，一个 2.30.0 读不出工具面变没变）。
**所以首屏写「记忆层 vX.Y.Z」而不是「Cinder vX.Y.Z」。**
③ **越界才提示，不拦人**：低于支持下界时给一句人话（「这版记忆层太旧，升级后再用记忆」），
**不硬拒绝**——硬拒绝会把版本耦合从"隐式"变成"显式但会拦人"，
而工具清单动态拉取已经覆盖了"新增工具"这一半。
④ **不做真能力协商**（客户端声明所需工具、逐个探测缺失）：它解决的是"工具被删/改名"，
频率极低，成本远高于收益——**这一半留给真出事的那天**。
⑤ **下界从契约版本的 major 号推**，不手抄一个魔数——主仓把契约版本**钉在工具清单旁边**
（`cmd/mcp/main_test.go` 的 `wantCoreVersion`），那边 CI 门禁保证「改了工具面必须涨版本号」，
所以本仓读到的那个号**本身就标着下界**：同一 major 之内向下兼容，跨 major 才需要重谈。

**已定的部分**：工具清单**动态拉取**（见上节），所以新增工具不会因为清单没更新而不可见——
**能力协商解决的是"工具被删了/改名了"，不是"工具新增了"**。

### 5. ~~copilot 要不要执行写操作~~（已定：分层——会话产物自动，长期知识过闸）

原问题：本仓定的是"走 MCP，只有一组写工具"（`notes_create` / `notes_update` / `notes_organize` / `handoff`）。
但**"要不要让 copilot 写笔记"是产品决定，不是技术决定**。

**已定（2026-11-05）：分层设闸。**

① **MCP 写面一共 6 个**：`notes_create` / `notes_update` / `notes_organize` / `handoff` / `scan` / `chat`
（`ai_review_rules` 实为只读）。其余 14 个都是读。
② **自动写**：`handoff`（会话交接——它本来就是给 agent 的写，ADR-0040 已把它放进证据层）、
`scan`（登记仓库，不是知识）、`chat`（追加会话消息——它是**会话产物**，而 `repotory_chat` 写的表
**不进检索**，所以这条写**影响不到 copilot 下一轮读到什么**，与 ③ 那三个是两类东西）。
③ **过一次人确认**：`notes_create` / `notes_update` / `notes_organize`——
**note 写完立刻进检索**（`SearchAll` 覆盖 notes），所以 copilot 能往**自己下一轮要读的记忆里**写东西；
ADR-0015 的原话正是这个形态（"model writes into the store with no gate"）。
**会话产物与长期知识不是同一类写**：前者自动、后者过闸。
④ **落地时机**：copilot 自己按名绑定的仍是两个——`repotory_chat`（共享会话传输）与 `repotory_metrics`（对话里问的那几个数）；**模型**在对话里可调的是只读白名单那八个（见「常驻形态」②），全部只读，不碰这条闸；长期知识那三个写工具对模型是**客户端拒绝**，一次都不会执行。
这条在 M2 收尾真要"回答写回 `repotory_handoff`"时落地。
⑤ **Phase 0 的「人工确认」从此覆盖两处**：白名单命令执行（Phase 3），
以及 copilot 通过 MCP 写长期知识（上面这三个）。
