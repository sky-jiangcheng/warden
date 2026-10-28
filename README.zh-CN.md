# Repotory Agent Shell

本地 Agent 入口。Shell 是入口，[Repotory](https://github.com/sky-jiangcheng/repotory) 是记忆层，MCP 是两者之间的开放协议。

> **当前阶段：Phase 0 — 仓库与定位。** 应用可构建、可运行；聊天链路在 Phase 2 落地。

## 这是什么

一个跑在你自己机器上的 Agent 外壳：打开一个仓库，用本地模型聊这个项目，让 Repotory 负责"记得什么"。

职责分得很清：

| | 归属 | 负责什么 |
|---|---|---|
| 入口 | Agent Shell（本仓库） | 本地模型入口 + 记忆面板 + 任务确认 |
| 记忆 | [Repotory](https://github.com/sky-jiangcheng/repotory) | 项目知识、会话交接、待确认知识 |
| 协议 | MCP | Shell 只通过 `repotory_*` 工具读记忆，不直连数据库 |

## 现在能跑什么

- 探测本机的模型运行时（Ollama 优先，llama.cpp / MLX 预留为插件式后端）
- 探测记忆层（Repotory MCP）是否就位
- 展示第一版产品边界与里程碑

聊天、命令执行、记忆面板都还没有 —— 见[路线](docs/roadmap.md)。

## 技术栈

Tauri v2 + Rust 后端 + TypeScript 前端。模型入口先选 Ollama，默认模型 `qwen3:8b`（Qwen3 8B Q4）。

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
