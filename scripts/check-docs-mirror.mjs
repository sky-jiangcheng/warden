#!/usr/bin/env node
// scripts/check-docs-mirror.mjs — 双语文档 1:1 镜像守卫（纯 node、零依赖）。
//
// 背景 / rationale：docs/**/*.md（中文源）与 docs/en/**/*.md（英文源）本应是互为
// 镜像的两棵树。只改一边 = 双语漂移：英文那半边会讲出不存在的产品架构。
// 本脚本把这件事变成一条硬门：任何一棵树里出现的 .md 文件，另一棵树必须有对应
// twin，否则构建直接失败。
//
// 运行：node scripts/check-docs-mirror.mjs
// 结果：全部 1:1 → exit 0；发现缺失/游离 → 打印清单并 exit 1。

import { readdirSync, existsSync, statSync } from 'node:fs'
import { join, relative, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const docsDir = join(root, 'docs')
const enDir = join(docsDir, 'en')

// 有意不出英文镜像的中文文档（相对 docs/ 的路径；以 '/' 结尾表示整棵子树）。
// 每条附理由，新增必须在此登记，否则守卫会把它当漂移报红。
const ZH_ONLY = []

// 递归列出目录下所有 .md（相对 baseDir 的 posix 风格路径）。
function listMarkdown(dir, baseDir) {
  const out = []
  if (!existsSync(dir)) return out
  for (const name of readdirSync(dir)) {
    const full = join(dir, name)
    const st = statSync(full)
    if (st.isDirectory()) {
      out.push(...listMarkdown(full, baseDir))
    } else if (name.endsWith('.md')) {
      out.push(relative(baseDir, full).split('\\').join('/'))
    }
  }
  return out
}

const zhSource = listMarkdown(docsDir, docsDir).filter((rel) => !rel.startsWith('en/'))
const enSource = listMarkdown(enDir, enDir)

function isZhOnly(rel) {
  return ZH_ONLY.some((entry) =>
    entry.endsWith('/') ? rel.startsWith(entry) : rel === entry
  )
}

const issues = []

// ① 中文源里、既不在白名单、又没有英文 twin 的文件 → 漏译。
for (const rel of zhSource) {
  if (isZhOnly(rel)) continue
  if (!enSource.includes(rel)) issues.push(`missing EN mirror: docs/${rel}  (docs/en/${rel})`)
}

// ② 英文源里、没有中文 twin 的文件 → 游离（英文那半边凭空多出一页）。
for (const rel of enSource) {
  if (!zhSource.includes(rel)) issues.push(`stray EN file: docs/en/${rel}  (no docs/${rel})`)
}

if (issues.length > 0) {
  console.error('docs mirror drift detected — ZH/EN .md trees are not 1:1:\n')
  for (const line of issues) console.error(`  - ${line}`)
  console.error(
    '\nFix: add the missing twin under docs/en/ (or docs/), or — if this file is ' +
      'intentionally Chinese-only — register it in ZH_ONLY above.'
  )
  process.exit(1)
}

console.log(`docs mirror OK: ${zhSource.length} ZH-source .md ↔ ${enSource.length} EN-source .md`)
