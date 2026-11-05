# Repotory Agent

一个跑在你自己机器上的 **copilot**：打开一个仓库，用本地模型聊这个项目，让 [Repotory](https://github.com/sky-jiangcheng/repotory) 负责"记得什么"。

> **当前阶段：Phase 2 — 模型 provider 可配置，聊天链路已通。** 应用可构建、可运行、能连上记忆层、能跟任何符合 OpenAI 兼容标准的模型对话。

## 这是什么

一个**独立产品**，不是 Repotory 桌面端的一个界面。

它做四件事：**自己认出本机的仓库**、**跟模型聊当前项目**、**在白名单内执行命令**、
**从 Repotory 取回记忆**。
模型由**你**挑——**任何符合 OpenAI 兼容标准的 provider 都能接**（OpenRouter、OpenAI、
DeepSeek、硅基流动、vLLM、LM Studio，本机的 Ollama 也一样）；界面形态也由你定。

职责分得很清：

| | 归属 | 负责什么 |
|---|---|---|
| copilot | Agent（本仓库） | 仓库发现 + 模型入口 + 命令执行 + 记忆面板 |
| 记忆 | [Repotory](https://github.com/sky-jiangcheng/repotory) | 项目知识、会话交接、已批准的知识页 |
| 协议 | MCP | Agent 只通过 `repotory_*` 工具读记忆，不直连数据库 |

## 它与 Repotory 桌面端的关系

**两个产品，各担一件事**——不是主从，不是同一产品的两个界面。

| | Repotory 桌面端 | Agent（copilot） |
|---|---|---|
| 定位 | **工作台** | **助手** |
| 干什么 | 浏览、编辑、**审批**、治理 | 读、聊、执行 |
| 记忆通道 | 直接用 `internal/service` | **只走 MCP** |
| 审批（approve / reject） | **有**（人在场） | **没有**——MCP 工具面里就没有这两个工具 |
| 关掉它 | 工作台没了 | copilot 照常跑 |
| 仓库从哪来 | 它自己的扫描根 | **它自己的扫描根 + 自己的收藏清单** |

**审批权留在人手里，而且不是靠约定维持的**：Repotory 的 MCP 工具面只注册了纯读的知识页工具
（`repotory_wiki_search` / `repotory_wiki_read`），写侧的闸门是「把内容发布进检索是人的行为」。
Agent 走 MCP，就自动站在闸门外。

**Agent 不直连数据库**：它需要 `repotory-mcp` 二进制存在。**没装 Repotory 就没有记忆**——
这一点在首屏就说清楚，不假装。

## 现在能跑什么

- **自己认仓库**：按你给的目录扫出 git 仓库，列成表，**新仓库从窗口顶部冒泡问你要不要收藏**；
  **忽略过的仓库不再问**
- **配置模型 provider**：填一个 base URL 就能接上任何 OpenAI 兼容的服务；
  **拉取模型列表**（服务端给什么就显示什么，**本仓不抄一份清单**）、
  **测试连接**（真发一次请求，验 key / 端点 / 模型三者都通）、选定模型
- **跟模型聊天**：回答是流式出来的，边写边显示
- **真的连上记忆层**：自动拉起 `repotory-mcp` 子进程走 stdio，做一次 MCP 握手，
  然后用 `tools/list` 拿服务端报的**真实工具清单**并在首屏列出来
  （本机实测 18 个工具——**本仓不抄一份清单**，服务端给什么就显示什么）
- 展示第一版产品边界与里程碑

命令执行、记忆面板还没有 —— 见[路线](docs/roadmap.md)。

## 仓库怎么认的

**copilot 自己扫盘，不经过 Repotory。** 给它几个要看的目录，它把里面的 git 仓库找出来，
列成一张表，**由你判断哪些是你在做的**。

- **仓库的判定**：目录里有 `.git` **目录**才算。worktree / submodule 的 `.git` 是**文件**，不算——
  这条规则跟 Repotory 主仓的扫描器一致，两边不会对同一目录给出不同的答案。
- **两条回答**：「收藏」是「这是我的活」，「忽略」是「别再问」。
  **忽略过的仓库永远不再冒泡**——否则说一次「不」还要被问第二遍。
- **新仓库冒泡**：扫到没处理过的仓库，窗口顶部滑出一个气泡问「是不是你的」，一个一个来
  （五个卡片一起问是表单，不是问题）。**窗口不在前台时改走系统通知，点通知把窗口叫回来。**
- **后台常驻**：窗口关掉也在扫（每 5 分钟一轮）。这才叫机器人，不是一张报表。
- **收藏清单存 copilot 自己**（app config 目录的 `repos.json`），**不写 Repotory 主库**：
  主库的 `projects.is_starred` 写入方是桌面端，MCP 工具面里没有孪生工具——
  要共享得先改主仓，那不是一个 copilot 该做的事。

列表现在只有名字与路径。**git 元信息（分支、最后提交时间）和「把仓库带进对话」还没做**。

## 模型 provider 怎么接的

**一套协议通吃**：OpenAI 兼容的 API 是这件事的事实标准，形状很小——

```
GET  {base}/models            → {"data": [{"id": "..."}, ...]}
POST {base}/chat/completions  → {"choices": [{"message": {"content": "..."}}]}
POST {base}/chat/completions  （"stream": true）→ SSE 行，data: [DONE] 收尾
```

OpenRouter、OpenAI、DeepSeek、硅基流动、vLLM、LM Studio、Ollama 的 `/v1` 面
全都说这套话，所以**代码里只有一个客户端**——加一家厂商是改配置，不是改代码。

**本机的 Ollama 走同一条路**（`http://127.0.0.1:11434/v1`）：
「本地模型」和「云端模型」在这里不是两类东西，是同一个形状。

**API Key 存进系统钥匙串**（macOS Keychain / Windows 凭据管理器 / Linux Secret Service），
配置文件里只存「有没有 Key」这一个事实——**明文落盘的密钥就是躺在每个备份里的密钥**。

**loopback 不走代理**：本机 provider 直连，其余走 `HTTPS_PROXY` / `HTTP_PROXY`（并尊重
`NO_PROXY`）。这不是偏好——开着 Clash 之类的代理时，代理是另一台机器，
它连 `127.0.0.1` 连的是它自己。

## 记忆层怎么接的

MCP stdio 就是**按行分隔的 JSON-RPC**。客户端的流程是三步：

```
initialize（带 id 的 request）
  → notifications/initialized（notification，没有 id）
  → tools/list / tools/call
```

`src-tauri/src/mcp_client.rs` 用 Rust 写的——**Tauri 的 webview 起不了子进程**，
而 IDE 插件能在 TypeScript 里做，是因为 VS Code / JetBrains 的宿主自带 Node / JVM runtime。
copilot 的等价宿主是 Rust 侧。

连接是**长驻**的：server 一直开着同一个 SQLite 库，每次调用重开等于每次重开库。
首屏分三态，不含糊：**未安装** / **装了但连不上**（给一个重试按钮） / **已连接 + N 个工具**。

## 技术栈

Tauri v2 + Rust 后端 + TypeScript 前端。默认 provider 是本机 Ollama，默认模型
`qwen3:8b`（Qwen3 8B Q4）——它不需要账号也不需要 Key，装好就能用。

**为什么是 Tauri**：它自带**托盘**（菜单栏图标 + 点击唤起），所以「关掉工作台还有一个助手在」
是做得出来的。（Wails v2 没有托盘，`pkg/menu` 里的
`TrayMenu` 是死类型，外部 `systray` 又要 cgo。）

**托盘已经落地**（`src-tauri/src/tray.rs`）：左键唤回窗口，右键是 Show / Quit 菜单。
**关窗口仍然结束应用**——托盘是给「已经在跑」的进程用的唤回入口，不是「窗口藏起来只留一个图标」
那种常驻。

**它是「独立进程 + 真托盘 + 真分发渠道」，不是原生 UI 框架**——Tauri 用的是系统 WebView，
不是 Swift/AppKit 控件。

## 开发

```bash
npm install
npm run tauri dev      # 开发
npm run tauri build    # 打包
```

Rust 后端单独类型检查：

```bash
cd src-tauri && cargo check
```

## 文档

- [定位与边界](docs/product.md) —— 第一版做什么、不做什么、有哪些待确认项
- [路线](docs/roadmap.md) —— Phase 0–6 与里程碑
- [AGENTS.md](AGENTS.md) —— 开发入口（给 AI agent 和新成员）

## 许可

MIT，见 [LICENSE](LICENSE)。
