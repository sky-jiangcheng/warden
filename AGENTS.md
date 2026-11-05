# AGENTS — 开发入口（面向 AI agent / 新成员）

Repotory Agent：一个跑在本机的 **copilot**，独立产品。copilot 是入口，[Repotory](https://github.com/sky-jiangcheng/repotory) 是记忆层，MCP 是两者之间的开放协议。

**它与 Repotory 桌面端是两个产品**：桌面端是**工作台**（浏览、编辑、**审批**、治理），
copilot 是**助手**（读、聊、执行）。**审批权留在人手里，而且不靠约定维持**——
Repotory 的 MCP 工具面在知识页上只有纯读工具，copilot 走 MCP 就自动站在 ADR-0015 的闸门外。

## 按需查文档（按此顺序，避免全文探索）

1. **[docs/product.md](docs/product.md)** — 定位、第一版边界、Phase 0 决议、待确认项
2. **[docs/roadmap.md](docs/roadmap.md)** — Phase 0–6 与里程碑
3. **[README.zh-CN.md](README.zh-CN.md)** — 项目是什么、怎么跑
4. **[Repotory 主仓的 ADR 索引](https://github.com/sky-jiangcheng/repotory/tree/master/docs/adr)** — 上游 ADR（检索/记忆/闸门规则的真相）；关键几篇：ADR-0047（本仓的定位来源）、ADR-0015（审批闸门）、ADR-0035（MCP 主通道）、ADR-0009（薄客户端纪律）

## 常用命令

```bash
npm install
npm run tauri dev        # 开发（Vite + Tauri 窗口）
npm run build            # 前端类型检查 + 构建（CI 门之一）
npm run check:docs       # 双语文档镜像守卫（CI 门之一）
cd src-tauri && cargo check     # Rust 后端类型检查
cd src-tauri && cargo fmt --all # 格式化（CI 门之一，提交前跑）
cd src-tauri && cargo clippy --all-targets -- -D warnings   # lint（CI 门之一）
cd src-tauri && cargo test --all-targets                    # 测试（CI 门之一）
```

## 目录结构

```
src/                 前端（TypeScript + Vite，无框架）
  main.ts            首屏渲染：只消费后端的 shell_status，不含任何运行时知识
  styles.css         设计令牌 + 卡片/徽章/排版
src-tauri/           后端（Rust）
  src/lib.rs         Tauri 命令装配与窗口生命周期
  src/tray.rs        托盘（copilot 的唤回入口）：左键唤回窗口，右键 Show / Quit
  src/model_backends.rs  模型运行时注册表（Ollama 已实现，llama.cpp / MLX 预留）
  src/memory_layer.rs    记忆层（Repotory MCP）探测；**工具名副本已漂，Phase 1 换成 tools/list 动态拉取**
docs/                中文文档源
docs/en/             英文文档源（与 docs/ 严格 1:1 镜像）
scripts/             仓库守卫脚本
```

## 约定

- **范围冻结**：Agent 不复制 Repotory 的检索逻辑、不直连它的数据库、不做完整 IDE、不做审批。这四条是产品边界，不是实现偏好——改边界要改 `docs/product.md`，不是改代码。
- **不越审批闸门**：知识页的 approve / reject 永远在 Repotory 桌面端做。copilot 走 MCP，工具面里本来就没有这两个工具——**不要为了「让面板更完整」去加绕过它的路径**。
- **模型请求一律走 Rust 后端**，前端不直连任何端口（Ollama、Repotory 服务都不例外）。
- **双语手册必须同批改**：`docs/**` 与 `docs/en/**` 是镜像，只改一边 = 双语漂移。守卫是 `npm run check:docs`（`scripts/check-docs-mirror.mjs`），有意只出中文的文档要在脚本的 `ZH_ONLY` 白名单登记并附理由。
- **提交规范**：conventional commits，`feat(scope): …` / `fix(scope): …` / `docs(scope): …` / `chore(scope): …`；一次提交只做一件事。
- **MCP 工具名的真相在 Repotory 主仓库**；`src/memory_layer.rs` 里那份只是首屏展示用的副本，**而且它已经漂了**（少列了多个真实工具）。**Phase 1 换成 `tools/list` 动态拉取**——一份会漂的清单比没有清单更坏，因为它让人以为那是真的。
- **「原生」这个词在这个仓里不成立**：Tauri 用系统 WebView，不是 Swift/AppKit 控件。copilot 是「独立进程 + 真托盘 + 真分发渠道」。**别在文档里把它写成原生应用。**

## 高频陷阱

- **`.gitignore` 已忽略 `src-tauri/target/`**，别把编译产物提上去；`node_modules`、`dist` 同理。
- **探测 ≠ 拉起**：当前后端只判断运行时/MCP 二进制**是否存在**，不会启动任何进程。真正拉起是 Phase 1 的事，别在 Phase 0 偷偷加自动启动。
- **后端注册表里 `planned` 的后端不能被前端当成可用**：UI 必须区分「可用 / 未安装 / 规划中」三态，否则会向用户承诺一个不存在的运行时。
- **图标已换成 Repotory 品牌标识**（OS 应用域 = squircle，与主仓 `build/appicon.png` 像素级一致）。
  品牌图标三域规则（母图 → 品牌域 / OS 应用域 / 字形域）在 Repotory 主仓库，
  **换图标要走那边的生成器**，别在本仓库手改 PNG。
- **CI 有三道门**（`.github/workflows/ci.yml`）：docs mirror / 前端 build / Rust fmt+clippy+test。
  **本地提交前把三条都跑一遍**——`cargo fmt` 尤其，它会改文件。
- **Rust 版本钉在 `rust-toolchain.toml`**（1.98.0），CI 与本地共用这一份真值。
  换版本要**故意**改，不要顺手改。
- **`cargo fmt` 会重排长行**：新增测试断言后如果 CI 的 fmt job 红了，
  跑 `cargo fmt --all` 而不是手改格式。
- **托盘是唤回入口，不是常驻**：关窗口仍然结束应用（与桌面端一致，ADR-0047 决策⑤）。
  托盘图标**必须在主线程创建**——`muda::Menu` 离开主线程会 panic，所以 `tray::install` 用
  `run_on_main_thread` 包一层。**tauri 的 `mock_app` 在单测里建不了真托盘**，
  正例只能靠运行时验证；单测只断言「未安装时 `tray.installed` 为 false」。
- **托盘图标素材**：macOS 用单色 template 图（`src-tauri/icons/tray-22.png`，22px——
  `tray-icon` 的 macOS 后端把图标固定缩到 18 points，44px 会被画成两倍高）；
  Windows / Linux 用彩色 app 图标（`32x32.png`）。**Tauri 只吃 PNG / ICO，不吃 SVG**。
