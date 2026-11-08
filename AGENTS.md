# AGENTS — 开发入口（面向 AI agent / 新成员）

Warden：一个跑在本机的 **copilot**（**不是** GitHub Copilot），独立产品。copilot 是入口，[Cinder](https://github.com/sky-jiangcheng/cinder) 是记忆层，MCP 是两者之间的开放协议。

**它与 Cinder Desktop 是两个产品**：桌面端是**工作台**（浏览、编辑、**审批**、治理），
copilot 是**助手**（读、聊、执行）。**审批权留在人手里，而且不靠约定维持**——
Cinder 的 MCP 工具面在知识页上只有纯读工具，copilot 走 MCP 就自动站在 ADR-0015 的闸门外。

## 按需查文档（按此顺序，避免全文探索）

1. **[docs/product.md](docs/product.md)** — 定位、第一版边界、Phase 0 决议、待确认项
2. **[docs/roadmap.md](docs/roadmap.md)** — Phase 0–6 与里程碑
3. **[README.zh-CN.md](README.zh-CN.md)** — 项目是什么、怎么跑
4. **[Cinder 主仓的 ADR 索引](https://github.com/sky-jiangcheng/cinder/tree/master/docs/adr)** — 上游 ADR（检索/记忆/闸门规则的真相）；关键几篇：ADR-0049（品牌重排与「显示层全换、标识层冻结」）、ADR-0047（本仓的定位来源）、ADR-0015（审批闸门）、ADR-0035（MCP 主通道）、ADR-0009（薄客户端纪律）

## 常用命令

```bash
npm install
npm run tauri dev        # 开发（Vite + Tauri 窗口）
npm run build            # 前端类型检查 + 构建（CI 门之一）
npm run check:docs       # 双语文档镜像守卫（CI 门之一）
npm run check:brand      # 品牌守门（CI 门之一）
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
  assets/warden.png  桌宠图：264px 光栅，按 80px 显示（3× 只为不留放大糊边）
src-tauri/           后端（Rust）
  src/lib.rs         Tauri 命令装配与窗口生命周期
  src/mcp_client.rs  MCP stdio 客户端：initialize / tools/list / tools/call
  src/tray.rs        托盘（copilot 的唤回入口）：左键唤回窗口，右键 Show / Quit
  src/binaries.rs    在本机找可执行文件（只找，不探活）
  src/providers.rs   provider 配置：persist + 同目录 0600 文件里的 API Key
  src/llm.rs         OpenAI 兼容客户端：拉模型 / 聊天 / SSE 流式
  src/provider_commands.rs  provider 命令背后的逻辑
  src/memory_layer.rs    记忆层（Cinder MCP）状态：连接 + 服务端给的工具清单
  src/repos.rs        仓库发现与收藏清单：扫盘 + watchlist 持久化
  tests/             端到端测试（真管道 + fake MCP server / fake LLM server / 真仓库目录）
docs/                中文文档源
docs/en/             英文文档源（与 docs/ 严格 1:1 镜像）
scripts/             仓库守卫脚本
```

## 约定

- **范围冻结**：Agent 不复制 Cinder 的检索逻辑、不直连它的数据库、不做完整 IDE、不做审批。这四条是产品边界，不是实现偏好——改边界要改 `docs/product.md`，不是改代码。
- **不越审批闸门**：知识页的 approve / reject 永远在 Cinder Desktop 做。copilot 走 MCP，工具面里本来就没有这两个工具——**不要为了「让面板更完整」去加绕过它的路径**。
- **模型请求一律走 Rust 后端**，前端不直连任何端口（Ollama、OpenRouter、Cinder 服务都不例外）。
- **模型 provider 是「一个 base URL」，不是一份厂商清单。**
  任何符合 OpenAI 兼容标准的 provider 都能接（OpenRouter / OpenAI / DeepSeek / 硅基流动 /
  vLLM / LM Studio / Ollama 的 `/v1` 面），所以**代码里只有一个客户端**。
  **加一家厂商是改配置不是改代码**——别为某家厂商开分支。
  **本机 Ollama 和远程 provider 走同一条路**（「本地模型」与「云端模型」不是两类东西）。
- **API Key 存同目录的 `secrets.json`，建文件即 0600**，provider 配置里只存「有没有 Key」。
  **别把 Key 混进 `providers.json`**——那个文件是给人看、给人贴进 issue 的。
  也别再改回系统钥匙串：ad-hoc 签名让 ACL 绑不住，「始终允许」每次都得重来
  （`src/providers.rs` 头上有完整推理）。
  前端 payload 里**永远只有 `hasApiKey`，没有 Key 本身**。
- **模型清单由服务端给出，本仓不保留副本**——跟 MCP 工具清单同一条规矩。
  拉到的清单不进配置文件（会漂），要用就现拉。
- **双语手册必须同批改**：`docs/**` 与 `docs/en/**` 是镜像，只改一边 = 双语漂移。守卫是 `npm run check:docs`（`scripts/check-docs-mirror.mjs`），有意只出中文的文档要在脚本的 `ZH_ONLY` 白名单登记并附理由。
- **品牌分两层：显示层全换、标识层冻结**（ADR-0049）。
  显示层是「用户看得见的名字」——本仓叫 **Warden**，桌面工作台叫 **Cinder Desktop**，母品牌叫 **Cinder**；真相源是本仓根目录的 `brand.json`（上游那份在主仓的 `build/brand.json`，**两处要同批改**，CI 各跑各的仓，谁也看不到对方）。
  标识层是「别人的机器认的东西」——`com.jiangcheng.repotoryagent`（Tauri 应用 id）、`repotory-agent`（crate / 二进制 / npm 包名）、`repotory-mcp`（它拉起的那个 MCP server）、`repotory_*`（工具名前缀）：**一个字都不许改**，改了是换接口，不是改名。
  **上游主仓的 GitHub slug 不在这张清单里**：它已于 2026-11-05 随母品牌 Cinder 改成 `sky-jiangcheng/cinder`（ADR-0049 补注二；改名前的地址记在那篇 ADR 里）。仓地址由 GitHub 自动重定向，老 clone 照常 fetch；但 **GitHub Pages 不随仓名重定向**（旧路径实测 404），所以文档站 URL 必须与改名同批改掉。
  守卫是 `npm run check:brand`（`scripts/check-brand.mjs`）：产品名必须等于 `brand.json`、冻结清单逐条仍在原处、当前面不留旧品牌词、标题改名后指向它的锚点也要跟着改。
  **要改品牌词：先改 `brand.json`，再改各处**，别反过来。
- **提交规范**：conventional commits，`feat(scope): …` / `fix(scope): …` / `docs(scope): …` / `chore(scope): …`；一次提交只做一件事。
- **MCP 工具名的真相在 Cinder 主仓库**，但**本仓库不再抄一份**：
  工具清单在连接时用 MCP 的 `tools/list` 动态拉取（`src/mcp_client.rs`），
  首屏显示的就是服务端刚报上来的那份。
  `src/memory_layer.rs` 里有一条自检测试，专门盯着别把工具名写回源码——
  它只扫字符串字面量，注释里可以提名（那是解释历史，不是契约）。
  **别再往任何前端文件里抄工具名**：那份副本已经漂过一次（少列 11 个）。
- **仓库是 copilot 自己认的**：扫盘在 `src/repos.rs`，**不借 Cinder 的手**——
  主仓那个扫描器是为它的知识库服务的，规则可以借用（`.git` 是目录才算），**代码不能复用**
  （那是它的库，copilot 不直连）。
  **收藏清单存 copilot 自己的 `repos.json`**（app config 目录），
  **不写主库的 `projects.is_starred`**——那个字段的写入方是桌面端（desktop-only），
  MCP 工具面里没有孪生工具，想共享得先改主仓。
- **「已处理」是收藏 ∪ 忽略**：一个仓库只有在这两个集合之外才算「新仓库」。
  **忽略过的仓库永远不再冒泡**——否则用户说一次「不」就要被问第二遍，这是最快让机器人变烦的办法。
- **首屏不能被可选能力阻塞**：记忆层是可选的，握手可能要十秒。
  `main()` 里**不要 `await` 它**——仓库列表、provider 列表各自独立跑。
  否则一个还没装的 `repotory-mcp` 会让整个首屏白等。
- **「原生」这个词在这个仓里不成立**：Tauri 用系统 WebView，不是 Swift/AppKit 控件。copilot 是「独立进程 + 真托盘 + 真分发渠道」。**别在文档里把它写成原生应用。**

## 高频陷阱

- **`.gitignore` 已忽略 `src-tauri/target/`**，别把编译产物提上去；`node_modules`、`dist` 同理。
- **MCP 走 stdio 时是「按行分隔的 JSON-RPC」，不是 LSP 那种 Content-Length 分帧。**
  请求一行 JSON、响应一行 JSON。服务端日志走 stderr（stdout 必须保持干净）。
  真相在 Cinder 主仓的 `mcp-go` 依赖里（`server/stdio.go`），不是猜的。
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
- **Tauri 的 `#[tauri::command]` 函数必须定义在 `lib.rs` 里**：`generate_handler!` 展开成
  `__cmd__<name>` 宏，而那些宏对定义它们的模块是私有的，跨模块引用报
  "macro import is private"。逻辑可以放别的模块，**命令外壳必须挨着 handler 列表**。
- **`reqwest` 的 TLS feature 叫 `rustls`**（0.13 已经删掉旧名 `rustls-tls`；
  `rustls-no-provider` 则要自己装 crypto provider）。`reqwest` 已经在依赖树里，
  但**只对 tauri 的 android/ios target 编译**——桌面端要用得自己写进 `Cargo.toml`。
- **loopback 绝不能走代理**：开着 Clash 之类的代理时（`HTTP_PROXY` 全局设着），
  代理是另一台机器，它的 `127.0.0.1` 是它自己——本机 Ollama 会被代理转出去然后失败。
  `llm.rs` 里 loopback 直连、其余走 `HTTPS_PROXY`/`HTTP_PROXY`（尊重 `NO_PROXY`）。
  **reqwest 默认的系统代理不绕过 loopback**，这条规则得自己实现。
- **前端 `pick()` 找不到元素会抛异常**——它不是 `querySelector` 的别名。
  **可选元素要用 `querySelector` 直取**：写成 `pick("#x")?.remove()` 里的 `?.` 根本轮不到
  （可选链在函数调用之后才生效），元素一不存在就炸。
- **流式输出用 `tauri::ipc::Channel`**（前端 `Channel` 类在 `@tauri-apps/api/core` 里），
  不要用返回值——返回值要让前端等整个回答，流式就没了。
- **后端注册表里 `planned` 的后端不能被前端当成可用**：UI 必须区分「可用 / 未安装 / 规划中」三态，否则会向用户承诺一个不存在的运行时。
- **图标是 Warden 的 8 字结标记**（ADR-0052「The Knot」：与 Cinder 同一条等宽带构造，只改圈数；母图在主仓 `build/icon-warden.svg`，Warden 是独立产品，不复用 Cinder 的三叶结）。
  品牌图标三域规则（母图 → 品牌域 / OS 应用域 / 字形域）在 Cinder 主仓库，
  **换图标要走那边的生成器，别在本仓库手改 PNG**：
  ```bash
  # 在主仓根目录跑，一条命令覆盖本仓的整套 Tauri 图标 + 托盘图
  node scripts/generate-icons.mjs --agent-repo ../warden
  ```
  本仓的图标**全部是生成物**（`src-tauri/icons/`：Tauri 的 PNG 阶梯 + Square* 商店阶梯 +
  `icon.ico` / `icon.icns` + `tray-22.png`）。改动 `icon-warden.svg` 后必须重跑这条命令，
  否则 app 会带着旧标记上线。
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
- **debug build 的窗口连的是 dev server，不是 `dist`**：`tauri.conf.json` 里的 `devUrl`
  （`http://localhost:1420`）在 debug 下生效。**裸 `cargo run` 而没起 `npm run dev` 时，
  窗口是空白的**——不是代码坏了，是没人监听那个端口。
  窗口里加一行 `.on_page_load` 日志（`[page] <url> Started/Finished`），
  它一句话就能把「白屏」分成「页面没加载」和「前端报错了」。
- **`cargo build` 不跟踪 `frontendDist` 的内容变化**：改了 `dist/` 之后重新 `cargo build`，
  cargo 认为没什么可做的（0.4 秒就完），二进制里嵌的**还是旧的资源**。
  `touch tauri.conf.json` 也不行——tauri-build 按配置的**内容哈希**判断要不要重新生成 codegen。
  **要重新嵌入就 `cargo clean -p repotory-agent && cargo build`**（或干脆用 `tauri dev`）。
- **插件要在 Builder 上注册，光有依赖和权限不够**：`tauri-plugin-notification` 加进 `Cargo.toml`、
  权限写进 `capabilities/default.json`，**但忘了 `.plugin(tauri_plugin_notification::init())`** 时，
  `app.notification()` 会 panic：`state() called before manage()`。
  **这条现在有测试守着**：Builder 抽成了共享函数 `builder<R>()`（`lib.rs`），
  测试用同一个函数建 app 再取 `notification()`——插件没注册就在这里 panic。
  **别把 Builder 改回 `run()` 里的一条链**，那样测试就看不见生产代码了。
- **macOS 默认菜单只能在主线程建**：`Builder::build` 在 macOS 上会建一个默认菜单，
  它走 `muda::Submenu` → 离开主线程就 panic（`can only be created on the main thread`）。
  **单测建 app 必须 `.enable_macos_default_menu(false)`**（tauri 自己的 `mock_builder()` 也这么做）。
  这跟通知插件无关，别为了它去改生产代码里的菜单行为。
- **`Builder::default()` 只对 `Builder<Wry>` 存在**（wry feature 开着的时候），
  泛型版本要用 `Builder::<R>::new()`。写 `tauri::Builder::default()` 再返回 `Builder<R>`
  会报类型不匹配。
- **真 app 冒烟的最小代价**：`npm run dev` + `HOME=/tmp/xxx cargo run`（隔离配置目录，
  别污染用户的 `~/Library/Application Support`），然后看 stderr 里有没有 `[page]` / `[repos]` 这几行。
- **`#[tauri::command]` 不能是 `pub fn`**：宏会为 pub 函数生成 `#[macro_export]` 的版本，
  和模块内的定义撞车，报 "the name `__cmd__xxx` is defined multiple times"。
  **命令保持私有，逻辑放 `pub` 的普通函数**（`repos.rs` 里的 `add_root_and_scan` 之类），
  端到端测试驱动那些函数——测试的就是命令要走的那条路。
- **仓库的判定：`.git` 是目录才算**，worktree / submodule 的 `.git` 是**文件**，不算。
  这条规则跟 Cinder 主仓的 scanner 一致（`internal/scanner/scanner.go`），
  **两边不能对同一目录给出不同答案**。
