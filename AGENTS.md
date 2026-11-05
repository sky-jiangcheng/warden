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
cd src-tauri && cargo clippy --all-targets --features test-fixtures -- -D warnings   # lint（CI 门之一）
cd src-tauri && cargo test --all-targets --features test-fixtures                   # 测试（CI 门之一）
```

`--features test-fixtures` 不是可选的：端到端测试要驱动 fake MCP server，
而它是**二进制**——不带这个 feature，Cargo 根本不编译它，测试也就找不到它。
这个 feature 同时把它挡在普通构建和发布产物之外（测试夹具不该进发布包）。

## 目录结构

```
src/                 前端（TypeScript + Vite，无框架）
  main.ts            首屏渲染：只消费后端的 shell_status，不含任何运行时知识
  styles.css         设计令牌 + 卡片/徽章/排版
src-tauri/           后端（Rust）
  src/lib.rs         Tauri 命令装配与窗口生命周期
  src/mcp_client.rs  MCP stdio 客户端：initialize / tools/list / tools/call
  src/tray.rs        托盘（copilot 的唤回入口）：左键唤回窗口，右键 Show / Quit
  src/model_backends.rs  模型运行时注册表（Ollama 已实现，llama.cpp / MLX 预留）
  src/memory_layer.rs    记忆层（Repotory MCP）状态：连接 + 服务端给的工具清单
  tests/             端到端测试（真管道 + fake MCP server）
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
- **MCP 工具名的真相在 Repotory 主仓库**，但**本仓库不再抄一份**：
  工具清单在连接时用 MCP 的 `tools/list` 动态拉取（`src/mcp_client.rs`），
  首屏显示的就是服务端刚报上来的那份。
  `src/memory_layer.rs` 里有一条自检测试，专门盯着别把工具名写回源码——
  它只扫字符串字面量，注释里可以提名（那是解释历史，不是契约）。
  **别再往任何前端文件里抄工具名**：那份副本已经漂过一次（少列 11 个）。
- **「原生」这个词在这个仓里不成立**：Tauri 用系统 WebView，不是 Swift/AppKit 控件。copilot 是「独立进程 + 真托盘 + 真分发渠道」。**别在文档里把它写成原生应用。**

## 高频陷阱

- **`.gitignore` 已忽略 `src-tauri/target/`**，别把编译产物提上去；`node_modules`、`dist` 同理。
- **MCP 走 stdio 时是「按行分隔的 JSON-RPC」，不是 LSP 那种 Content-Length 分帧。**
  请求一行 JSON、响应一行 JSON。服务端日志走 stderr（stdout 必须保持干净）。
  真相在 Repotory 主仓的 `mcp-go` 依赖里（`server/stdio.go`），不是猜的。
- **握手是三步，顺序不能错**：`initialize`（带 id 的 request）→ `notifications/initialized`
  （**notification，没有 id，不能用 request 发**，否则永远等不到回答）→ `tools/list` / `tools/call`。
- **Tauri 的 async command 不能带 `State<'_, T>` 参数**：那个 future 满足不了 `'static` bound，
  编译期就报 `__tauri_message__ does not live long enough`。
  改用 `AppHandle` + `Manager::state()` 取。顺带：async command 只要有引用参数就**必须返回 `Result`**。
- **async command 里不要做阻塞 IO**：Tauri 的同步命令跑在 IPC 线程上，阻塞 60 秒会把窗口冻住。
  MCP 的读侧是普通 std 线程（子进程 stdout 是阻塞管道，在 tokio worker 里读会拖垮整个 worker），
  等答案那侧用 tokio `oneshot` + `time::timeout`（非阻塞）。
- **`CARGO_BIN_EXE_<name>` 只对 `[[bin]]` 生效，对 `[[test]]` 是 `NotPresent`。**
  所以 fake MCP server 必须是 `[[bin]]`（配 `required-features = ["test-fixtures"]` 把它挡在发布包外），
  路径在**运行时**读 `std::env::var("CARGO_BIN_EXE_fake-mcp-server")`——
  `env!` 在编译期就展开，那里拿不到。
- **Rust 测试 harness 会往 stdout 打 `running 1 test`**，那会插进 JSON-RPC 流里。
  fake server 因此必须是独立 bin（`test = false`），且它自己只打协议。
- **加第二个 `[[bin]]` 之后 `cargo run` 会认不出该跑哪个**，
  报 "could not determine which binary to run"——`Cargo.toml` 里加 `default-run = "repotory-agent"`。
- **现在真的会拉起进程了**（Phase 1）：MCP client 启动 `repotory-mcp` 子进程，
  一次握手然后长驻。**别把它改回「只探测不拉起」**——
  首屏要分三态：未安装 / 装了但连不上 / 已连接 + 服务端给的工具清单。
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
