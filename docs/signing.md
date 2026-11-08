# macOS 签名与公证

> 装 Cinder / Warden 时 macOS 弹「Apple 无法验证"xxx"是否包含可能危害 mac 安全或泄露隐私的恶意软件」——
> 这不是 App 有问题，是**构建没有 Apple Developer ID 签名，也没有公证**。这一页讲怎么把它彻底关掉。

---

## 一、先搞清楚三件事分别是什么

| 现象 | 它是什么 | 谁能解决 |
| :--- | :--- | :--- |
| 「未打开"xxx"。Apple 无法验证…」 | **Gatekeeper**拦下未签名/未公证的 App | 签名 + 公证 |
| 「"xxx"已损坏，无法打开」 | 同上，措辞不同 | 签名 + 公证 |
| 钥匙串问密码 | 钥匙串条目的 ACL | 与本文无关，见[排障](../troubleshooting.md) |

签名是**证明是谁做的**，公证是**Apple 扫过一遍证明不是恶意软件**。两个都要有：只有签名，弹窗会从「无法验证开发者」变成「已损坏」；只有公证没签名，一样拦。

**公证的前提是签名，而且签名必须带 hardened runtime 和安全时间戳**——这三条缺一条，Apple 会直接拒收。

---

## 二、拿证书

### 2.1 你需要 Developer Program 会员

**免费 Apple ID 不行。** 需要 [Apple Developer Program](https://developer.apple.com/programs/) 会员，99 美元/年（个人或公司都可以）。
中国大陆的账号在付款环节经常失败，可以改用**手机号注册**（选「没有美国区 App Store 账号时」→ 选中国区手机号），这条路成功率更高。

会员开通后 [developer.apple.com/account](https://developer.apple.com/account/) 就能进后台了。

### 2.2 拿到 Team ID

后台右上角头像 →「Membership details」→ **Team ID**，10 位大写字母数字（如 `ABCDE12345`）。**这个值后面到处都要用。**

### 2.3 创建 Developer ID Application 证书

**方式 A：Xcode（推荐，最省事）**

1. Xcode → Settings → Accounts → 选你的 Apple ID → Manage Certificates… → 左下角 **+** → 选 **Developer ID Application** → Done
2. 证书和私钥自动进了你的钥匙串

验证：

```bash
security find-identity -v -p codesigning | grep "Developer ID Application"
```

看到类似这样的输出就成了：

```
1) A1B2C3D4E5F6... "Developer ID Application: 你的名字 (ABCDE12345)"
```

**方式 B：开发者后台（没有 Xcode 时用）**

1. [Certificates 页面](https://developer.apple.com/account/resources/certificates) → 左上角 **+** → Software 选 **Developer ID Application** → Continue
2. 需要一个 **CSR 文件**。macOS 上用「钥匙串访问 → 证书助理 → 创建证书颁发请求」生成，存成 `.certSigningRequest`
3. 上传 CSR → Continue → Download，拿到 `.cer`
4. 双击 `.cer` 装进钥匙串

> ⚠️ 创建 Developer ID 证书需要 **Account Holder** 角色。团队里别人是 Admin 的话看不到这个选项。

---

## 三、导出成 CI 能用的形式

CI 里不能读你的钥匙串，得把证书导出来。

### 3.1 导出 .p12

钥匙串访问 → 找到 **Developer ID Application** 那张 → 双击 → 展开「导出」箭头 → 导出为 **PKCS#12 (.p12)** → **设置一个密码**（这个密码后面要用，别丢）。

然后转成 base64（GitHub Secrets 存 base64，避免换行符把 YAML 搞坏）：

```bash
openssl base64 -in ~/Desktop/DeveloperIDApplication.p12 -out ~/Desktop/cert-base64.txt
```

### 3.2 拿到签名身份名

就是 2.3 里 `security find-identity` 输出的那段**引号里的完整字符串**：

```
"Developer ID Application: 你的名字 (ABCDE12345)"
```

这是 `APPLE_SIGNING_IDENTITY` 的值。**带引号内容本身，不带外面的引号。**

---

## 四、公证凭据（二选一）

> **在哪台机器上做都一样。** 公证跑在 GitHub Actions 的 macOS runner 上，本机只负责把下面几样东西写进
> Secrets。换一台「有密钥」的机器不会让这一步变简单——真正决定成败的是凭据本身对不对。

### 方案 A：App Store Connect API Key（推荐）

CI 里最稳：没有密码进日志，发版时也不会弹两步验证。

1. [App Store Connect → 用户和访问 → 集成](https://appstoreconnect.apple.com/access/integrations/api) → **+** → 选 **App Store Connect** → 命名 → 权限选 **Developer** → Continue → **Download**（`.p8` 文件，**只能下载一次**）
2. 同一页面上方能看到 **Issuer ID**（UUID）

**这里要的是「团队密钥」（Team Key），不是「个人 API 密钥」（Individual API Key）。** Apple 官方原话：个人密钥
「无法使用 Provisioning 接口、访问 Sales and Finance，或 **notaryTool**」——拿个人密钥去公证会直接失败，
而报错通常不会告诉你是这个原因。「用户和访问 → 集成」页面上这两类密钥是**两个分区**，点 **+** 之前先看清分区标题。

于是你有三样东西：

- `APPLE_API_KEY` = Key ID（那串 10 位字母数字）
- `APPLE_API_ISSUER` = Issuer ID
- `APPLE_API_KEY_P8` = `.p8` 文件的 **base64**

```bash
openssl base64 -in ~/Downloads/AuthKey_XXXXXXXXXX.p8 -out ~/Desktop/p8-base64.txt
```

### 方案 B：Apple ID + app 专用密码

这条路**不挑密钥类型**：个人 API 密钥不能用于公证，但 Apple ID + app 专用密码可以。手上那份 `.p8` 要是个人
密钥，不用再去找别的机器，直接走这条路——一个网页、一次点击就完事。

1. [appleid.apple.com](https://appleid.apple.com) → 登录 → 「登录与安全」→ **App 专用密码** → 创建一个 → 得到 16 位密码（`abcd-efgh-ijkl-mnop` 形式，**只显示一次**）
2. `APPLE_ID` = 你的 Apple ID 邮箱
3. `APPLE_PASSWORD` = 那个 app 专用密码
4. `APPLE_TEAM_ID` = 2.2 里的 Team ID

> App 专用密码**只能用于公证**，不能用它登录开发者后台。

---

## 五、配进 GitHub Secrets

**两个仓库都要配一遍**（Cinder 和 Warden 各有自己的发布流水线）：

```bash
# 在 Warden 仓库
cd /path/to/warden
gh secret set APPLE_CERTIFICATE        < ~/Desktop/cert-base64.txt
gh secret set APPLE_CERTIFICATE_PASSWORD --body '<你导出 .p12 时设的密码>'
gh secret set APPLE_SIGNING_IDENTITY    --body 'Developer ID Application: 你的名字 (ABCDE12345)'
gh secret set APPLE_TEAM_ID             --body 'ABCDE12345'

# 方案 A 的三个
gh secret set APPLE_API_KEY             --body 'XXXXXXXXXX'
gh secret set APPLE_API_ISSUER          --body 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx'
gh secret set APPLE_API_KEY_P8          < ~/Desktop/p8-base64.txt

# 或者方案 B 的两个（不要和方案 A 混用）
gh secret set APPLE_ID                  --body 'you@example.com'
gh secret set APPLE_PASSWORD            --body 'abcd-efgh-ijkl-mnop'
```

`gh secret set` 读文件时要加 `<`，给值时用 `--body`。**密码类不要写进命令行历史**——用 `--body` 也会进 shell history，更安全的写法：

```bash
gh secret set APPLE_PASSWORD < <(security find-generic-password -s ... -w)
```

或者直接用交互式输入（`gh secret set APPLE_PASSWORD` 不带参数会提示你输入，不回显）。

### 配完确认

```bash
gh secret list | grep APPLE
```

---

## 六、CI 里发生了什么

Warden 用 Tauri，签名和公证由 **Tauri 的 bundler** 在 `tauri build` 里一并完成——不需要自己写
脚本。流水线做的是把凭据喂给它：

```
publish job
  ├─ Configure Apple signing      ← 把 secrets 灌进 $GITHUB_ENV，写出 .p8 文件
  ├─ Verify notarisation creds    ← 半配置就当场报错，不等到构建末尾
  ├─ Build and publish (tauri-action)
  │    └─ tauri build
  │         ├─ 导入证书到临时钥匙串（并设好 partition list）
  │         ├─ 签 .app
  │         ├─ notarytool submit --wait
  │         └─ stapler staple
  └─ Notarise the disk image
       ├─ notarytool submit .dmg --wait
       ├─ stapler staple .dmg
       └─ gh release upload --clobber ← 换掉 tauri-action 传上去的那一份
```

**为什么要多这一步**：Tauri 的 bundler 只公证 `.app`，**不公证 `.dmg`**——但用户下载的是 dmg，
Gatekeeper 先查的也是 dmg。app 里有票据、外层 dmg 没有，用户照样弹「Apple 无法验证」。
所以 `.dmg` 得自己再公证一次、staple 一次，再换回 Release 上那一份。

Tauri 读的变量名就是上面那五个：`APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、
`APPLE_SIGNING_IDENTITY`，以及公证用的 `APPLE_API_KEY` / `APPLE_API_ISSUER` /
`APPLE_API_KEY_PATH`（或 `APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID`）。

**每一步失败都会让 job 失败。** 这不是保守——一个没公证的 dmg 照样能装上，只是每个人打开时都弹一次框；这种失败在发布流水线上是看不见的，只有用户会替你发现。

---

## 七、没配证书会怎样

**证书和公证凭据都没配**：不会导致发版失败，会打一条 warning，产物是未签名的。

**证书配了但公证凭据没配**：发版**直接失败**，预检当场报错。这是故意的——签了名没公证的产物照样能装上，
只是每个人打开都弹一次框，而这种失败在发布流水线上看不见，只有用户会替你发现。

未签名的 dmg 怎么绕过：

```bash
xattr -dr com.apple.quarantine "/Applications/Warden.app"
```

或者右键 →「打开」→ 再点一次「打开」。或者用 `curl` 的一键脚本装——`curl` 不写隔离标记，装完直接能开。

> **别用 `sudo spctl --master-disable`。** 那是把整台机器的下载检查关掉，拿所有 App 的风险换一个 App 的方便。

---

## 八、常见问题

**「the imported certificate does not provide identity」**
`APPLE_SIGNING_IDENTITY` 和证书对不上。用 `security find-identity -v -p codesigning` 看准确的字符串（引号里的内容，不带引号）。

**「security import」失败，提示密码不对**
`APPLE_CERTIFICATE_PASSWORD` 是**导出 .p12 时**设的密码，不是 Apple ID 密码。

**`codesign` 一直挂住不动**
`set-key-partition-list` 那行没执行到。它让 codesign 不用问「是否允许使用这个私钥」。

**公证报「The executable does not have a secure timestamp」**
签名时少了 `--timestamp`。

**公证报「The binary is not signed with the hardened runtime」**
签名时少了 `--options runtime`。

**公证报「The signature does not include a secure timestamp」**
同上，或者签名时用的不是 Developer ID Application 证书。

**公证报「package does not contain a valid signature」**
签的是 dmg 而不是 app 里的可执行文件。要从内到外签：先签 `Contents/MacOS/` 里的东西，再签 `.app`。

**App Store Connect API Key 报「The issuer ID is invalid」**
`APPLE_API_ISSUER` 填的是 Issuer ID（UUID），不是 Key ID。两个别弄反。

**换了证书怎么办**
旧证书过期前签的 App 还能继续用（只要用户装的时候证书还没过期）。重新签之前先删掉旧的 `v*` 标签再打新的。

---

## 相关页面

- [排障手册](../troubleshooting.md) —— 钥匙串弹窗、Key 存在哪、401/404
- [快速上手](getting-started.md) —— 安装
