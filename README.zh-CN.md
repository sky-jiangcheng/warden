<p align="center">
  <a href="https://github.com/sky-jiangcheng/warden"><img src="src-tauri/icons/128x128.png" width="96" alt="Warden" /></a>
</p>

<h1 align="center">Warden</h1>

<p align="center">
  <b>编辑器关掉之后，还在那儿的那个 copilot。</b><br/>
  菜单栏里的本地助手。它自己认出本机的仓库，桌上留一盏灯和一句话，<br/>
  跟你聊当前这个项目——走 MCP，模型由你挑。
</p>

<p align="center">
  <b>两个产品，各担一件事。</b><br/>
  <a href="https://github.com/sky-jiangcheng/cinder">Cinder</a> Desktop 是<b>工作台</b>——浏览、编辑、<b>审批</b>、治理这份记忆。<br/>
  Warden 是<b>助手</b>——读、聊、执行。共享同一份本地记忆、同一套协议；<br/>
  <b>审批永远在 Cinder Desktop</b>，因为审批工具根本不在 Warden 能用的 MCP 工具面里。
</p>

<p align="center">
  <a href="https://github.com/sky-jiangcheng/warden/releases"><img src="https://img.shields.io/github/v/release/sky-jiangcheng/warden?color=6366f1" alt="Release" /></a>
  <a href="https://github.com/sky-jiangcheng/warden/actions/workflows/ci.yml"><img src="https://github.com/sky-jiangcheng/warden/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <img src="https://img.shields.io/badge/Platforms-macOS%20%7C%20Windows%20%7C%20Linux-111827" alt="macOS | Windows | Linux" />
  <img src="https://img.shields.io/badge/Account-not%20required-22c55e" alt="不需要账号" />
  <img src="https://img.shields.io/badge/Model-any%20OpenAI--compatible-7c3aed" alt="任何 OpenAI 兼容模型" />
  <a href="./LICENSE"><img src="https://img.shields.io/github/license/sky-jiangcheng/warden?color=22c55e" alt="MIT License" /></a>
</p>

<p align="center">
  <a href="README.md">English</a> | 简体中文 · <a href="docs/product.md">定位</a> · <a href="docs/roadmap.md">路线</a> · <a href="docs/troubleshooting.md">排障</a>
</p>

---

## 🎯 为什么还要第二个 App？

因为你一关编辑器，每个编码 agent 就把这个项目忘了——而你想要的助手，本来就不该只属于一个窗口。

| 今天的毛病 | Warden 的做法 |
| :--- | :--- |
| 🪟 **助手长在编辑器里**。窗口一关，它和那段对话一起没了。 | **常驻进程 + 真托盘**。关窗口是隐藏，托盘 Quit 才退出；窗口在后台时，一轮正在生成的回答照样往下写。 |
| 🌙 **「我昨天干到哪了」丢了**。昨天的决定、踩过的坑、下一步——一个会话结束就全没了。 | **它读的就是你的 agent 写进去的那份记忆**。项目知识、会话交接、已批准的知识页，全部通过 MCP 从 Cinder 来——是 Cursor 写过的那条记录，不是一份长得像的第二份历史。 |
| 🧠 **长回答要大模型，随手一问不要**。 | **任何 OpenAI 兼容的 provider**。代码里只有一个客户端、没有一家厂商的专有代码：本机 Ollama 免费离线，问题难了换 OpenRouter / OpenAI / DeepSeek。换只是改一个 base URL。 |
| 📂 **每个工具都要你告诉它代码在哪**。 | **它自己扫盘**。给它几个目录，它把下面的 git 仓库找出来；新仓库从窗口顶部冒泡问「是不是你的」，**忽略一次就永远不再问**。 |

---

## 📦 两个产品

不是主从，也不是同一个应用的两个窗口——是同一个闭环的两半。

| | Cinder Desktop | Warden |
| :--- | :--- | :--- |
| 定位 | **工作台** | **助手** |
| 干什么 | 浏览、编辑、**审批**、治理 | 读、聊、执行 |
| 记忆通道 | 直接用 service 层 | **只走 MCP**，绝不直连数据库 |
| 审批（approve / reject） | **有**——人在场 | **没有**——那两个工具根本不在 Warden 的 MCP 工具面里 |
| 关掉它 | 工作台没了 | copilot 照常跑 |
| 仓库从哪来 | 它自己的扫描根 | **它自己的扫描根 + 自己的收藏清单** |

**审批权留在人手里，是靠结构而不是靠约定维持的**：Cinder 的 MCP 工具面只注册了纯读的知识页工具，
写侧的闸门是「把内容发布进检索是人的行为」。Warden 走 MCP，就自动站在闸门外——没有哪个开关可以被设错。

**没有 `repotory-mcp` 就没有记忆。** Warden 把 MCP server 当子进程拉起来。那个二进制本身就是每个平台
一份的独立 release 产物，所以**桌面 App 不是必需品**；首屏会把三种状态说清楚，不假装。

---

## 🚀 安装

### macOS

从 [Releases](https://github.com/sky-jiangcheng/warden/releases) 下 `Warden_<版本>_universal.dmg`
（一份通用包，Apple silicon 和 Intel 都能用），拖进 `/Applications`，首次打开前清掉隔离标记：

```bash
xattr -dr com.apple.quarantine "/Applications/Warden.app"
```

这份构建**没有 Apple Developer ID 签名、也没公证**，所以 macOS 会拿
「Apple 无法验证 "Warden" 是否包含可能危害 Mac 安全或泄露隐私的恶意软件」拦下来。
上面那条命令就是绕过这个对话框的正确做法；[为什么，以及什么才是真正的解法](docs/troubleshooting.md#macos-打不开应用)。

### Windows · Linux

| 平台 | 产物 | 说明 |
| :--- | :--- | :--- |
| Windows | `Warden_<版本>_x64_en-US.msi` / `_x64-setup.exe` | 未签名，SmartScreen 可能要求确认 |
| Linux | `Warden_<版本>_amd64.AppImage` / `Warden_<版本>_amd64.deb` / `Warden-<版本>-1.x86_64.rpm` | AppImage 免安装 |

### 记忆层的前置条件

装完就能聊——只要选个 provider。**记忆**相关的东西需要 `PATH` 上有 `repotory-mcp` 二进制，它来自
[Cinder](https://github.com/sky-jiangcheng/cinder) 的 release，或 Cinder Desktop 的安装包。
没有它，首屏显示「未安装」，其余功能照常。

---

## ⚡ 三步上手

1. **打开 Warden。** 没有账号、没有登录、没有上报。它直接待在托盘里。
2. **选模型。** 默认是本机 Ollama（`http://127.0.0.1:11434/v1`，模型 `qwen3:8b`）——不要 Key，
   Ollama 在跑就等于配好了。要换别的：粘一个 base URL，点「测试连接」（真发一次请求，
   端点、Key、模型三者一起验），挑模型，完事。
3. **给它一个目录。** 它把下面的 git 仓库列出来，新发现的一个一个问你「是不是你的」。
4. **问。** 点那个小伴侣，或按 `⌥Space`。回答边写边出来。

---

## ✅ 现在能跑什么

- **自己认仓库**——按你给的目录每 5 分钟扫一轮，窗口关着也扫；新仓库冒泡（窗口不在前台时改走系统通知），
  **忽略过的永远不再冒泡**
- **配置任何 OpenAI 兼容 provider**——base URL、向服务端拉模型清单（Warden 不抄任何人的目录）、
  真发一次请求的连通测试、可选附加 header
- **流式聊天**，而且这条会话**和 IDE 插件看到的是同一份**：主题与消息 2.5 秒轮询 `repotory_chat`，
  Warden、VS Code、JetBrains 读写同一条记录。只同步已完成的消息，消息不可变（同 id 不同载荷报错，
  绝不覆盖），每条助手消息按它自己的 `source` 标来源——**把插件的回复标成 Warden 是替别人认领答案**
- **常驻小伴侣窗**——132×128，里面只有灯体和一句气泡；点它或按 `⌥Space` 开聊天，只有灯体拖得动窗口。
  **关窗口是隐藏**（红按钮不该掐断一轮正在生成的回答），**托盘 Quit 才真的退出**
- **数字按需取**——在对话里问一句，模型去调 `repotory_metrics`，读的是记忆里**已经存着**的数：
  不扫盘、不跑 git、不刷缓存。覆盖不到就显示 `null`，**不显示一个猜出来的数**
- **真的连上记忆层**——走 stdio 拉起 MCP server，做一次握手，再用 `tools/list` 拿服务端报的
  **真实工具清单**并显示（服务端给什么就是什么，Warden 不抄一份）

**还没做**：在仓库里执行命令、记忆面板、仓库列表上的 git 元信息（分支、最后提交）。
顺序都在[路线](docs/roadmap.md)里写着，不美化。

---

## 🧭 它是怎么接的

### 三条通道

| 通道 | 归谁 | 怎么走 | 备注 |
| :--- | :--- | :--- | :--- |
| 仓库 | Warden | 自己的扫描器（app config 目录的 `repos.json`） | 有 `.git` **目录**才算仓库，worktree / submodule 的 `.git` **文件**不算——这条规则和 Cinder 一致，两边不会对同一目录给出不同答案 |
| provider | Warden | HTTPS 打你配的 base URL，loopback 直连 | **本机 provider 不走代理**：开着代理时，代理是另一台机器，它的 `127.0.0.1` 是它自己 |
| 记忆 | [Cinder](https://github.com/sky-jiangcheng/cinder) | MCP over stdio，只用 `repotory_*` 工具 | 连接是**长驻**的：server 一直握着同一个 SQLite 句柄，每次调用重连等于每次重开库 |

### 一套协议通吃所有模型

```
GET  {base}/models            → {"data": [{"id": "..."}, ...]}
POST {base}/chat/completions  → {"choices": [{"message": {"content": "..."}}]}
POST {base}/chat/completions  （"stream": true）→ SSE 行，data: [DONE] 收尾
```

OpenRouter、OpenAI、DeepSeek、硅基流动、vLLM、LM Studio、Ollama 的 `/v1` 面全都说这套话，
所以**代码里只有一个客户端**——加一家厂商是改配置，不是改代码。「本地模型」和「云端模型」
在这里不是两类东西，是同一个形状。

### 为什么是 Tauri

Warden 需要**托盘**——菜单栏图标、点一下唤起——因为「关掉工作台还有一个助手在」就是整个产品。
Wails v2 没有托盘（`pkg/menu` 里的 `TrayMenu` 是死类型），外部 `systray` 又要 cgo，
会把 Cinder CI 里那句裸的 `go build ./...` 弄坏。Tauri 还顺带给小伴侣窗留下了真正的前端工具链。

它是**独立进程 + 真托盘 + 真分发渠道**，不是原生 UI 框架：Tauri 用系统 WebView，
所以界面是 Web 技术，不是 Swift/AppKit 控件。

---

## ❓常见问题

**必须装 Cinder Desktop 吗？**
不用。记忆这条通道需要 `PATH` 上有 `repotory-mcp`；其余（仓库、provider、聊天）都不需要它，
而且首屏会直说，不会等你在第一个问题上撞墙。

**我的代码和问题会离开这台机器吗？**
只发往**你自己**配的那个 provider。没有 Warden 的服务器、没有账号、没有遥测、没有崩溃上报。
用默认的 Ollama，什么都不会离开本机。

**API Key 存在哪？**
存在 provider 列表旁边的 `secrets.json`，权限 `0600`——**不在 macOS 钥匙串里**。
这是个有意的取舍，取舍的另一面和替代方案写在
[API Key 为什么要落成文件](docs/troubleshooting.md#api-key-为什么落成文件)。

**macOS 说无法验证这个 App，是正常的吗？**
正常——这份构建只有 ad-hoc 签名。见 [macOS 打不开应用](docs/troubleshooting.md#macos-打不开应用)。

**为什么不做成 IDE 插件？**
因为插件会跟着宿主一起死，而真正有意思的时刻恰恰是没有编辑器开着的那个时刻——你在终端里看栈，
或者想先问一句「重试策略当初是怎么定的」再决定开什么。Warden 是那个一直在跑的；IDE 插件是来来去去的那批。
它们共享的是记忆，不是进程。

---

## 🛠 开发

```bash
npm install
npm run tauri dev      # 开发（Vite + Tauri 一条命令）
npm run tauri build    # 打一个本机平台的包
```

后端单独跑，不需要窗口服务：

```bash
cd src-tauri && cargo check --all-targets --features test-fixtures
cd src-tauri && cargo test --features test-fixtures     # 含端到端 MCP 那一轮
```

CI 强制、本地也能跑的门：

```bash
node scripts/check-brand.mjs        # 显示层品牌词、冻结的标识层、仓地址
node scripts/check-docs-mirror.mjs  # docs/*.md 与 docs/en/*.md 1:1
cargo fmt --all -- --check
cargo clippy --all-targets --features test-fixtures -- -D warnings
```

---

## 📚 文档

- [定位与边界](docs/product.md) —— 第一版做什么、不做什么、有哪些待确认项
- [路线](docs/roadmap.md) —— Phase 0–6 与里程碑
- [排障](docs/troubleshooting.md) —— Gatekeeper 对话框、Key 文件、找不到 MCP、provider 报错
- [AGENTS.md](AGENTS.md) —— 开发入口（给 AI agent 和新成员）

---

## 📄 许可

[MIT](LICENSE) © 2026-present [sky-jiangcheng](https://github.com/sky-jiangcheng) and Warden contributors.

Warden 在 MIT 许可下**个人和商业用途都免费**。具体到实际意思：

| 你想 | MIT 允许吗 |
| :--- | :--- |
| 在公司里用，跑在商业代码上 | ✅ 可以 |
| fork 下来改，改动不公开 | ✅ 可以 |
| 打包进你卖的产品里 | ✅ 可以 |
| 把改过的构建分发给团队 | ✅ 可以 |

许可证唯一没有交出去的是**名字**：`Warden` 与 `Cinder` 的名称和标识仍归本项目所有，
所以 fork 不能自称 Warden——改个名、保留版权声明、链回来就行。限制就这一条。

**第三方组件。** 应用打包了 [Tauri](https://tauri.app)（MIT / Apache-2.0）以及 `src-tauri/Cargo.lock`
里那些 Rust crate（多为 MIT / Apache-2.0），它们的许可归各自作者。**不打包任何模型权重**：
Warden 自己不带模型，你连的那个模型用什么许可，是你和它的提供方之间的事。
它拉起的 `repotory-mcp` 属于 [Cinder](https://github.com/sky-jiangcheng/cinder)，同一个 MIT。

**没有担保。** 正如 MIT 原文所说，软件按「现状」提供。它对仓库只读，只在自己的配置目录里写东西，
但它是 1.0 之前的软件——锁一个你试过的版本，升级前看一眼 release notes。
