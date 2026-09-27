#!/usr/bin/env node
/**
 * Assemble the versioned documentation site.
 *
 * What the world reads must be what the checks guard: the published set is the
 * DOCUMENTS list out of `tools/check-golden-path.mjs`, read mechanically rather than
 * restated, so a document can neither be published uncheckable nor checked unpublished
 * (§78's two spellings, refused in advance). If that list cannot be parsed, this fails
 * loudly instead of publishing a guess.
 *
 *     node tools/build-docs-site.mjs --tag v0.1.0 --out _site [--existing versions.json]
 *
 * Layout:
 *
 *     _site/index.html            the version list, linking latest/
 *     _site/versions.json         ["v0.1.0", ...], newest last
 *     _site/latest/               this tag's documents, as committed
 *     _site/versions/<tag>/       the same, pinned to the tag
 *
 * `--existing` is the live site's versions.json (fetched by the workflow); absent on
 * the first deploy. GitHub Pages renders the Markdown; index.html passes through.
 */

import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = resolve(dirname(fileURLToPath(import.meta.url)), '..')

function flag(name) {
  const index = process.argv.indexOf(name)
  return index === -1 ? null : process.argv[index + 1]
}

/** The checked set, read out of the checker rather than restated here. */
function checkedDocuments() {
  const source = readFileSync(join(HERE, 'tools', 'check-golden-path.mjs'), 'utf8')
  const block = /const DOCUMENTS = \[([\s\S]*?)\n\]/.exec(source)
  if (!block) {
    console.error('build-docs-site: cannot find DOCUMENTS in tools/check-golden-path.mjs, '
      + 'so the published set is unknown. Refusing to publish a guess.')
    process.exit(1)
  }
  // Element lines only, not comments: `// §65's path` contains an apostrophe pair that a
  // bare quote-match reads as a document, which is exactly the false name this refuses to
  // publish. Lines starting with `//` can never be elements.
  const documents = block[1].split('\n')
    .map(line => /^\s*'([^']+)'\s*,?\s*$/.exec(line))
    .filter(match => match)
    .map(match => match[1])
  if (documents.length === 0) {
    console.error('build-docs-site: DOCUMENTS parsed to nothing. Refusing to publish.')
    process.exit(1)
  }
  return documents
}

function escapeHtml(text) {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

function main() {
  const tag = flag('--tag')
  const out = flag('--out')
  if (!tag || !out) {
    console.error('usage: node tools/build-docs-site.mjs --tag v0.1.0 --out _site '
      + '[--existing versions.json]')
    process.exit(1)
  }
  if (!/^v\d+\.\d+\.\d+/.test(tag)) {
    console.error(`build-docs-site: ${tag} is not a release tag (v<workspace version>). `
      + 'Docs versions are releases; anything else would be a version nobody can install.')
    process.exit(1)
  }

  const documents = checkedDocuments()
  const missing = documents.filter(document => !existsSync(join(HERE, document)))
  if (missing.length > 0) {
    console.error(`build-docs-site: the checked set names files that are not here: `
      + missing.join(', '))
    process.exit(1)
  }

  let versions = []
  const existing = flag('--existing')
  if (existing && existsSync(existing)) {
    try {
      const parsed = JSON.parse(readFileSync(existing, 'utf8'))
      if (Array.isArray(parsed)) versions = parsed.filter(v => typeof v === 'string')
    } catch {
      console.error('build-docs-site: --existing is not JSON. Refusing to merge it.')
      process.exit(1)
    }
  }
  if (!versions.includes(tag)) versions.push(tag)

  const site = resolve(out)
  for (const directory of [`versions/${tag}`, 'latest']) {
    for (const document of documents) {
      const target = join(site, directory, document)
      mkdirSync(dirname(target), { recursive: true })
      copyFileSync(join(HERE, document), target)
    }
  }
  // The README is the front page of every version, and directory indexes are Jekyll's.
  copyFileSync(join(site, 'latest', 'README.md'), join(site, 'latest', 'index.md'))

  const items = [...versions].reverse()
    .map(version => `    <li><a href="versions/${escapeHtml(version)}/">${escapeHtml(version)}</a></li>`)
    .join('\n')
  writeFileSync(join(site, 'index.html'),
    `<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n`
    + `<meta http-equiv="refresh" content="0; url=latest/">\n`
    + `<title>Skynet OS documentation</title>\n</head>\n<body>\n`
    + `<h1>Skynet OS documentation</h1>\n`
    + `<p>Latest: <a href="latest/">${escapeHtml(tag)}</a>.</p>\n<ul>\n${items}\n</ul>\n`
    + `</body>\n</html>\n`)
  writeFileSync(join(site, 'versions.json'), JSON.stringify(versions, null, 2) + '\n')

  console.log(`site for ${tag}: ${documents.length} documents, `
    + `${versions.length} version(s) indexed (${versions.join(', ')})`)
}

main()
