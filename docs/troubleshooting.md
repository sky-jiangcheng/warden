# 排障

先看这一页，再看 issue。这里列的都是「看起来像坏了、其实是设计如此」或者「一条命令就能过」的事。

- [macOS 打不开应用](#macos-打不开应用)
- [API Key 为什么落成文件](#api-key-为什么落成文件)
- [升级之后聊不了了：Key 要重填一次](#升级之后聊不了了key-要重填一次)
- [首屏说「未安装」或一直连不上](#首屏说未安装或一直连不上)
- [provider 报 401 / 404 / 超时](#provider-报-401--404--超时)
- [对话和 IDE 插件不同步](#对话和-ide-插件不同步)
- [关窗口没退出](#关窗口没退出)

---

## macOS 打不开应用

**症状**：双击 App 或挂载的 dmg，弹出

> 「Warden」已损坏，无法打开。你应该将它移到废纸篓。

或者

> 未打开 "Warden"。Apple 无法验证 "Warden" 是否包含可能危害 Mac 安全或泄露隐私的恶意软件。

**为什么**：macOS 的 Gatekeeper 只放行两类东西——App Store 的，或者用 Apple Developer ID
签名并**公证（notarise）**过的。**v0.1.8 起 Warden 两个都是**：dmg 用 Developer ID 签名、
提交过 Apple 公证、票据 stapled 在包里。v0.1.8 之前只有 ad-hoc 签名（`codesign -s -`，
签名主体就是这段二进制本身），再加上浏览器下载时给 dmg 打了 `com.apple.quarantine` 标记，
首次启动就会撞上这个对话框。

**怎么办**（只有旧版本需要，任选一种，都是一次性的）：

```bash
# 装到 /Applications 之后，清掉隔离标记
xattr -dr com.apple.quarantine "/Applications/Warden.app"
```

或者在 Finder 里**右键 → 打开**，在对话框里再点一次「打开」。

> **别关 Gatekeeper。** `sudo spctl --master-disable` 会让整台机器的下载检查失效，
> 那是拿一个 App 换掉所有 App 的保护。上面那条命令只动这一个 bundle。

**新版本为什么不用管**：签名（`codesign --options runtime --timestamp`）证明是谁做的，
公证（`xcrun notarytool submit --wait` + `xcrun stapler staple`）是 Apple 扫过一遍证明不是
恶意软件。两个都做完，用户双击就开，没有任何对话框——`release.yml` 里每次都自动走。
完整的步骤（怎么拿证书、怎么配 secrets）在[签名与公证](signing.md)。

Windows 同理：没有代码签名证书，SmartScreen 会拦一次，点「更多信息 → 仍要运行」即可。

---

## API Key 为什么落成文件

**症状**：升级到 v0.1.7 之后，每次启动 macOS 都弹出（旧版本的行为）

> Warden 想要使用你储存在钥匙串的 "repotory-agent" 中的机密信息。若要给予许可，请输入"登录"钥匙串的密码。

点「始终允许」也没用，下次还问。

**为什么**：钥匙串条目的访问控制列表（ACL）绑的是**创建它的那个 App 的代码签名**。
Warden 是 ad-hoc 签名的，而 ad-hoc 签名就是这段二进制的哈希——**每出一个新构建，在钥匙串眼里
就是一个全新的 App**，「始终允许」这条授权没法挂到它身上，于是再问一遍。

一个每次启动都要你输一次登录密码的凭据存储，不是在保护密钥，是在训练你闭着眼点「允许」。
所以 v0.1.7 起，provider 的 API Key 存到 app config 目录里的 `secrets.json`，
**创建时就是 `0600`**（不是先创建再 chmod，中间没有可读窗口）。

**取舍，说清楚**：落在文件里的密钥，会进你的备份、进同步目录、进屏幕共享。
换来的是：读它不需要任何授权、能用 `cat` 看一眼、能用 `rm` 删掉、和这个 App 的其他状态受同一层保护。
不想让任何密钥落盘的话，用本机 Ollama——它不需要密钥。

两个文件都在同一个目录：

```
macOS    ~/Library/Application Support/com.jiangcheng.repotoryagent/
Windows  %APPDATA%\com.jiangcheng.repotoryagent\
Linux    ~/.config/com.jiangcheng.repotoryagent/
```

- `providers.json` —— 名字、base URL、默认模型、附加 header；**不含 Key**，只记「有没有 Key」这一个事实
- `secrets.json` —— `{"provider-id": "sk-..."}`，`0600`

旧的钥匙串条目还在（macOS 钥匙串本身不会自己清理）。想删干净：
打开「钥匙串访问」→ 搜 `repotory-agent` → 删除。Warden 不再读写它。

---

## 升级之后聊不了了：Key 要重填一次

**症状**：从 v0.1.6 及更早升级上来，第一次发消息报

> the provider's API key is missing — open the provider settings and save it again

**为什么**：旧版本的 Key 存在系统钥匙串里，新版本不读钥匙串了（见上一节）。迁移没有做，
因为**读一次旧条目就等于再弹一次那个密码框**——为了不弹，只能不读。

**怎么办**：打开设置 → 「连接」→ 那个 provider → 把 API Key 重新粘一次 → 保存。一次就够，之后都不再问。

---

## 首屏说「未安装」或一直连不上

记忆这条通道需要一个二进制：`repotory-mcp`。它来自 [Cinder](https://github.com/sky-jiangcheng/cinder)
的 release 产物（每个平台一份，不需要装桌面 App），或者 Cinder Desktop 的安装包。

```bash
which repotory-mcp          # 在 PATH 上吗
repotory-mcp --version      # 能跑起来吗
```

首屏区分三态，别混着读：

| 状态 | 意思 | 怎么办 |
| :--- | :--- | :--- |
| 未安装 | `PATH` 上找不到这个二进制 | 装它，或把它的目录加进 `PATH` |
| 装了但没应答 | 找到了，握手失败或超时 | 点重试；在终端里直接跑一次看它报什么 |
| 已连接 + N 个工具 | 通 | —— |

**GUI 应用的 `PATH` 和你终端里的不一样。** 从 Finder / Dock 启动的进程不读你的 `.zshrc`，
所以「终端里 `which repotory-mcp` 有、Warden 说没有」是常见情况：把二进制放到 `/usr/local/bin`
或 `~/.local/bin` 这类系统路径下，别只放在某个 shell 才会加载的目录里。

---

## provider 报 401 / 404 / 超时

| 现象 | 大概率是 | 怎么办 |
| :--- | :--- | :--- |
| `404 Not Found` | base URL 少了路径段。填的是站点根地址，而要的是 API 根地址 | 补成 `https://host/v1` 这种形状，再点「测试连接」 |
| `401` / `403` | Key 不对、过期，或者这个 Key 没有这个模型的权限 | 重存 Key；确认模型 id 拼写 |
| 连不上 / 超时 | 本机 provider 被代理拦了，或者服务没起来 | Warden 对 loopback 直连、不走代理；先 `curl http://127.0.0.1:11434/v1/models` 试一下 |
| 模型列表是空的 | 这个服务不实现 `GET /models` | 手动填模型 id，再用「测试连接」验 |

「测试连接」发的是**真的聊天请求**，不是 ping——它同时证明端点、Key、模型三件事。所以它失败的时候，
基本就是这三个里有一个不对，按上表顺序查。

---

## 对话和 IDE 插件不同步

两件事先分清：

- **有共享主题的对话**会同步：主题与消息轮询记忆层的 `repotory_chat`，Warden、VS Code、JetBrains
  读写同一条记录。
- **没有共享主题的那段是临时对话**，只留在当前窗口，状态条上就写着它没同步。
- **只有已完成的消息落库**。一轮还在流式生成时，另一边看不到它——那不是丢消息。
- **消息不可变**：同一个 id 换一份载荷是报错，不是覆盖。

所以「这边发了、那边没看到」，先看状态条是不是临时对话，再看那条消息是不是还没生成完。

---

## 关窗口没退出

**设计如此。** 红按钮只隐藏窗口：一轮正在生成的回答不该被它掐断，而且 Warden 是 `repotory-mcp`
的父进程，退出会连带断掉那条常驻会话通道。

**要退出**：点托盘图标 → Quit（或 `⌘Q`）。这跟 Cinder Desktop 正好相反——桌面端关窗口即退出，
因为它不托管任何长驻通道。
