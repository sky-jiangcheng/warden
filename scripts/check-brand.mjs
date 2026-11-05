#!/usr/bin/env node
// scripts/check-brand.mjs — 品牌守门（CI gate，纯 node、零 npm 依赖）。
//
// 背景 / rationale：ADR-0049 把品牌重排成三层——母品牌 Cinder、桌面工作台
// Cinder Desktop、本仓的助手 Warden——同时划了一条线：**显示层全换、标识层冻结**。
//
// 这条线两边都会漂，而且方向相反：显示层漏一处，用户就看到一个旧名字（本仓的
// 首屏曾经写着「Repotory Agent」就是这么来的）；标识层被「顺手清理」一处，每个
// 用户的 MCP 客户端配置就断一次。所以这里查四件事：
//
//   ① 品牌词有唯一真相源（brand.json），各处产品名必须与它相等
//   ② 标识层冻结清单逐条仍在原处——冻结不是「不检查」，是「检查它还在」
//   ③ 当前面（用户看得见的面）不再出现旧品牌词
//   ④ 标题里的品牌词改了，指向它的锚点也得跟着改——否则文档链接指向空处
//
// 依赖：git（只扫已提交的文件——未提交的本地中间物不该决定门红不红）。
//
// 运行：node scripts/check-brand.mjs
// 结果：全过 → exit 0；发现漂移 → 打印清单并 exit 1。

import { readFileSync, existsSync, readdirSync, statSync } from 'node:fs'
import { join, dirname, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const issues = []

function read(rel) {
  return readFileSync(join(root, rel), 'utf8')
}

function report(where, expected, got) {
  issues.push(`${where}: 期望 ${expected}，实际 ${got === undefined ? '读不到' : got}`)
}

/** A word standing on its own — "Warden" must not match "Wardens". */
function hasWord(text, word) {
  return new RegExp(`(?<![A-Za-z0-9_])${word}(?![A-Za-z0-9_])`).test(text)
}

// ------------------------------------------------- ① 品牌词的唯一真相源 ----

const brandPath = 'brand.json'
if (!existsSync(join(root, brandPath))) {
  console.error(`ERROR: ${brandPath} 不存在——它是品牌词的 SSOT`)
  process.exit(1)
}
const brand = JSON.parse(read(brandPath))
for (const key of ['mother', 'desktop', 'agent', 'idePlugin']) {
  if (!brand[key] || typeof brand[key] !== 'string') {
    issues.push(`${brandPath}: 读不到 "${key}"`)
  }
}

// The product name the OS shows and the title of the window are exact fields,
// not prose: a near miss there is a wrong product name in the user's dock.
const tauriConf = existsSync(join(root, 'src-tauri/tauri.conf.json'))
  ? JSON.parse(read('src-tauri/tauri.conf.json'))
  : null
if (!tauriConf) {
  issues.push('src-tauri/tauri.conf.json: 文件不存在或不是合法 JSON')
} else {
  if (tauriConf.productName !== brand.agent) {
    report('src-tauri/tauri.conf.json · productName', brand.agent, tauriConf.productName)
  }
  const win = tauriConf.app?.windows?.[0]
  if (!win) {
    issues.push('src-tauri/tauri.conf.json: 读不到 app.windows[0]')
  } else if (win.title !== brand.agent) {
    report('src-tauri/tauri.conf.json · app.windows[0].title', brand.agent, win.title)
  }
}

const indexHtml = existsSync(join(root, 'index.html')) ? read('index.html') : null
if (!indexHtml) {
  issues.push('index.html: 文件不存在')
} else {
  const title = indexHtml.match(/<title>([^<]*)<\/title>/)?.[1]
  if (title !== brand.agent) {
    report('index.html · <title>', brand.agent, title)
  }
  const h1 = indexHtml.match(/class="brand__name">([^<]*)</)?.[1]
  if (h1 !== brand.agent) {
    report('index.html · 首屏产品名', brand.agent, h1)
  }
}

// The rest of the surfaces are prose, so they only have to carry the word.
// A site that names the wrong product is worse than a site that names
// nothing, because the user believes it.
const BRAND_SITES = [
  ['README.md', 'agent', 'README 首屏的产品名'],
  ['README.zh-CN.md', 'agent', 'README 首屏的产品名'],
  ['AGENTS.md', 'agent', 'AGENTS.md 的产品名'],
  ['docs/product.md', 'agent', '定位文档首句'],
  ['docs/en/product.md', 'agent', 'positioning doc, first line'],
  ['src-tauri/src/lib.rs', 'agent', 'crate 头的自述'],
]

// The other product must be named by its full name wherever the two products
// are compared — "Cinder" alone reads as the brand family, and the family is
// not what the user is being asked to open.
const SIBLING_SITES = [
  ['README.md', 'desktop', '两个产品各担一件事'],
  ['README.zh-CN.md', 'desktop', '两个产品各担一件事'],
  ['AGENTS.md', 'desktop', '它与 Cinder Desktop 是两个产品'],
  ['docs/product.md', 'desktop', '与 Cinder Desktop 的关系'],
  ['docs/en/product.md', 'desktop', 'how it relates to the Cinder Desktop app'],
  ['docs/roadmap.md', 'desktop', '它与 Cinder Desktop 是两个产品'],
  ['docs/en/roadmap.md', 'desktop', 'two products'],
]

for (const [file, key, where] of [...BRAND_SITES, ...SIBLING_SITES]) {
  const word = brand[key]
  if (!word) continue
  if (!existsSync(join(root, file))) {
    issues.push(`${file}: 文件不存在（${where}）`)
    continue
  }
  if (!hasWord(read(file), word)) {
    issues.push(`${file}: ${where}里没有品牌词「${word}」`)
  }
}

// The desktop name is the mother name plus a qualifier. If that stops
// holding, "Cinder Desktop" is two unrelated words and the family reads
// as two brands.
if (brand.mother && brand.desktop && !brand.desktop.startsWith(`${brand.mother} `)) {
  issues.push(
    `brand.json: 桌面名「${brand.desktop}」不是母品牌「${brand.mother}」加后缀——` +
      '一个品牌族里的两个面要读得出同源'
  )
}

// The IDE plugins' marketplace name lives upstream, in the main
// repository. This repository carries the word so the two copies of the
// source of truth cannot disagree about the shape of it — and so the
// rule is the same on both sides: the qualifier is what keeps the name
// inside the family.
if (brand.mother && brand.idePlugin && !brand.idePlugin.startsWith(`${brand.mother} `)) {
  issues.push(
    `brand.json: 插件名「${brand.idePlugin}」不是母品牌「${brand.mother}」加后缀——` +
      'Marketplace 上它是独立条目，名字得让人看得出它属于这个品牌族'
  )
}

// ------------------------------------------------- ② 标识层冻结清单 ----

// The interface layer, frozen by ADR-0049 decision 3. Each entry must
// still be there, verbatim, in the file that owns it. "Frozen" is not
// "don't look" — it is "look, and fail if it moved".
const FROZEN = [
  [
    'src-tauri/tauri.conf.json',
    '"identifier": "com.jiangcheng.repotoryagent"',
    'Tauri 应用 id（装完就是系统里的 app identity，改了等于换了个 App）',
  ],
  ['src-tauri/Cargo.toml', 'name = "repotory-agent"', 'crate 与二进制名（cargo run / cargo clean -p 都按它找）'],
  ['package.json', '"name": "repotory-agent"', 'npm 包名'],
  [
    'src-tauri/src/memory_layer.rs',
    'const MCP_BINARY: &str = "repotory-mcp";',
    '它拉起的 MCP server 二进制名（用户 PATH 上那个）',
  ],
  ['src-tauri/tests/mcp_stdio.rs', '"repotory_context"', 'MCP 工具名前缀契约（主仓冻结的 repotory_ 前缀）'],
  ['AGENTS.md', 'https://github.com/sky-jiangcheng/repotory', '上游主仓的 GitHub slug'],
]
for (const [file, literal, where] of FROZEN) {
  if (!existsSync(join(root, file))) {
    issues.push(`${file}: 文件不存在（${where}）`)
    continue
  }
  if (!read(file).includes(literal)) {
    issues.push(`${file}: 冻结的标识层不见了 —— ${where}（期望字面量 ${literal}）`)
  }
}

// ------------------------------------------------- ③ 当前面不留旧品牌词 ----

// The old brand word, standing on its own. Class names (RepotoryClient) are
// code identifiers and are out of scope: renaming them is churn, not
// branding.
const OLD_BRAND = 'Repotory'
const oldWordRe = new RegExp(`(?<![A-Za-z0-9_])${OLD_BRAND}(?![A-Za-z0-9_])`)

// Comment lines are not a surface: they explain why a frozen string still
// says "repotory", and stripping them keeps that explanation from being
// mistaken for a display-layer miss.
const COMMENT_EXT = new Set(['.ts', '.tsx', '.js', '.mjs', '.css', '.yml', '.yaml', '.sh', '.ps1'])

// Binary assets cannot be scanned as text. The icons are covered by the main
// repository's generator check instead — that is where the master mark lives.
const TEXT_EXT = new Set([
  '.md', '.ts', '.tsx', '.js', '.mjs', '.json', '.rs', '.toml', '.css',
  '.html', '.yml', '.yaml', '.sh', '.ps1', '.txt', '.svg',
])

// The scanner's own source contains the word it hunts for. Excluding it by
// path is honest: a word in the scanner is not a brand leak.
const SELF = 'scripts/check-brand.mjs'

// The screenshot archive shows the UI as it was on the day it was taken —
// before the rebrand, so the old product name is visible in the pixels. That
// is evidence, not a leak: the archive's own README says so out loud. The
// exemption is deliberately narrow (the archive only), so a current-facing
// document cannot hide an old name under it.
const HISTORICAL = ['outputs/screenshots/']

function stripComments(text, ext) {
  if (!COMMENT_EXT.has(ext)) return text
  return text
    .replace(/\/\*[\s\S]*?\*\//g, ' ')
    .split('\n')
    .map((line) => {
      const hash = line.indexOf('#')
      const slash = line.indexOf('//')
      let cut = -1
      if (slash >= 0) cut = slash
      if (hash >= 0 && (cut < 0 || hash < cut)) cut = hash
      return cut >= 0 ? line.slice(0, cut) : line
    })
    .join('\n')
}

// Frozen literals explain any occurrence of the old word that contains
// one of them — that occurrence IS the interface, not a leftover name.
const frozenLiterals = FROZEN.map(([, literal]) => literal)

let tracked = []
try {
  tracked = execFileSync('git', ['ls-files', '-z'], { cwd: root, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 })
    .split('\0')
    .filter(Boolean)
} catch (err) {
  console.error('ERROR: git ls-files 失败——本检查只扫已提交的文件，需要在 git 工作树里跑')
  process.exit(1)
}

const scanned = tracked.filter(
  (f) =>
    f !== SELF &&
    !HISTORICAL.some((h) => f === h || f.startsWith(h)) &&
    TEXT_EXT.has(f.slice(f.lastIndexOf('.')))
)
for (const file of scanned) {
  if (!existsSync(join(root, file))) continue
  const ext = file.slice(file.lastIndexOf('.'))
  const text = stripComments(read(file), ext)
  const lines = text.split('\n')
  lines.forEach((line, i) => {
    if (!oldWordRe.test(line)) return
    if (frozenLiterals.some((lit) => line.includes(lit))) return
    issues.push(`${file}:${i + 1}: 当前面还留着旧品牌词「${OLD_BRAND}」—— ${line.trim().slice(0, 90)}`)
  })
}

// ------------------------------------------- ④ 标题改名 → 锚点跟着改 ----

// A markdown anchor is derived from the heading text. When the heading's
// brand word changed, the anchor changed with it — and every link that
// pointed at the old slug pointed at nothing. The rename broke four of
// them before anything noticed, because a broken anchor still renders as
// ordinary text.
//
// The comparison ignores punctuation: this repository truncates anchors at
// the first parenthetical ("#4-version-coupling-with-cinder"), so the rule
// is "the anchor must be a prefix of the heading's slug", not "equal".
function slug(text) {
  return text
    .toLowerCase()
    .trim()
    .replace(/[\p{P}\p{S}]/gu, (c) => (c === '-' || c === '_' ? c : ''))
    .replace(/\s/g, '-')
}

function loose(text) {
  return text.toLowerCase().replace(/[^\p{L}\p{N}]/gu, '')
}

function listMarkdown(dir, base) {
  const out = []
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules' || name === 'target' || name === '.git') continue
    const full = join(dir, name)
    if (statSync(full).isDirectory()) out.push(...listMarkdown(full, base))
    else if (name.endsWith('.md')) out.push(relative(base, full).split('\\').join('/'))
  }
  return out
}

const mdFiles = listMarkdown(root, root)
const slugsByFile = new Map()
for (const file of mdFiles) {
  const set = new Set()
  for (const line of read(file).split('\n')) {
    const m = line.match(/^(#{1,6})\s+(.*)$/)
    if (!m) continue
    set.add(slug(m[2].replace(/`/g, '')))
    set.add(slug(m[2]))
  }
  slugsByFile.set(file, set)
}

for (const file of mdFiles) {
  const lines = read(file).split('\n')
  lines.forEach((line, i) => {
    const re = /\[[^\]]*\]\(([^)\s]+)\)/g
    let m
    while ((m = re.exec(line)) !== null) {
      const target = m[1]
      if (/^(https?:|mailto:|file:)/.test(target)) continue
      const [filePart, anchor] = target.split('#')
      if (!anchor) continue
      const targetFile = filePart ? join(dirname(file), filePart).split('\\').join('/') : file
      if (!existsSync(join(root, targetFile))) {
        issues.push(`${file}:${i + 1}: 链接指向不存在的文件 —— ${target}`)
        continue
      }
      const slugs = slugsByFile.get(targetFile)
      if (!slugs) continue
      const wanted = loose(anchor)
      const hit = [...slugs].some((s) => {
        const have = loose(s)
        return have === wanted || have.startsWith(wanted)
      })
      if (!hit) {
        issues.push(
          `${file}:${i + 1}: 锚点指向空处 —— ${target}（标题改了，锚点里的旧品牌词也得改）`
        )
      }
    }
  })
}

// ------------------------------------------------------------------ 结论 ----

if (issues.length > 0) {
  console.error(`品牌漂移检测到 ${issues.length} 处：\n`)
  for (const line of issues) console.error(`  - ${line}`)
  console.error(
    '\n修法：显示层改成 brand.json 里的词；标识层不许动——' +
      '它是接口不是品牌（ADR-0049 决策③），要动先走契约 major bump。'
  )
  process.exit(1)
}

console.log(
  `brand OK: 母品牌 ${brand.mother} / 桌面 ${brand.desktop} / 助手 ${brand.agent}；` +
    `${BRAND_SITES.length + SIBLING_SITES.length} 处产品名一致，${FROZEN.length} 条冻结标识层仍在原处，` +
    `${scanned.length} 个当前面文件无旧品牌词，${mdFiles.length} 个文档的锚点都还指得到标题。`
)
