# Repotory Agent

一个跑在你自己机器上的 **copilot**：打开一个仓库，用本地模型聊这个项目，让 [Repotory](https://github.com/sky-jiangcheng/repotory) 负责"记得什么"。

> **当前阶段：Phase 0 — 仓库与定位。** 应用可构建、可运行；聊天链路在 Phase 2 落地。

## 这是什么

一个**独立产品**，不是 Repotory 桌面端的一个界面。

它做三件事：**跟本地模型聊当前项目**、**在白名单内执行命令**、**从 Repotory 取回记忆**。
模型后端由**你**挑（Ollama 起，llama.cpp / MLX 走插件式后端）；界面形态也由你定。

职责分得很清：

| | 归属 | 负责什么 |
|---|---|---|
| copilot | Agent（本仓库） | 本地模型入口 + 命令执行 + 记忆面板 |
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

**审批权留在人手里，而且不是靠约定维持的**：Repotory 的 MCP 工具面只注册了纯读的知识页工具
（`repotory_wiki_search` / `repotory_wiki_read`），写侧的闸门是「把内容发布进检索是人的行为」。
Agent 走 MCP，就自动站在闸门外。

**Agent 不直连数据库**：它需要 `repotory-mcp` 二进制存在。**没装 Repotory 就没有记忆**——
这一点在首屏就说清楚，不假装。

## 现在能跑什么

- 探测本机的模型运行时（Ollama 优先，llama.cpp / MLX 预留为插件式后端）
- 探测记忆层（Repotory MCP）是否就位
- 展示第一版产品边界与里程碑

聊天、命令执行、记忆面板都还没有 —— 见[路线](docs/roadmap.md)。

## 技术栈

Tauri v2 + Rust 后端 + TypeScript 前端。模型入口先选 Ollama，默认模型 `qwen3:8b`（Qwen3 8B Q4）。

**为什么是 Tauri**：它自带**托盘**（菜单栏图标 + 点击唤起 + 后台常驻），
所以「关掉工作台还有一个助手在」是做得出来的。（Wails v2 没有托盘，`pkg/menu` 里的
`TrayMenu` 是死类型，外部 `systray` 又要 cgo。）

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
