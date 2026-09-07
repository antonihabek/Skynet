/**
 * Where the Hub's checkout is, when nobody said — in one place, because it was in three.
 *
 * # Why this is a module and not seven lines repeated
 *
 * Three tools in this directory need the same answer: `check-mirrors.mjs` compares the shared
 * documents, `check-contract-mirrors.mjs` compares the mirrored contracts, and `scale-report.mjs`
 * measures the Hub's mirror for one of §73's figures. All three resolved it themselves, and on
 * 2026-09-07 they disagreed:
 *
 * | Tool | Spellings it looked for |
 * |---|---|
 * | `check-mirrors.mjs` | seven |
 * | `check-contract-mirrors.mjs` | seven |
 * | `scale-report.mjs` | **two** |
 *
 * So on a machine holding both trees at `../RobotOverflow`, two of the three found the Hub and the
 * third printed *"the Hub's mirror was not measured: no checkout beside this one"* — and went on
 * reporting the one §73 figure that spans both repositories as unmeasured, indefinitely, with a
 * green exit code.
 *
 * That is §78's shape exactly: one fact, several spellings, agreeing until they do not. §78's own
 * rule is that the remedy is a mechanism rather than care, and the mechanism for a list is that
 * there is one list.
 *
 * # The failure this is arranged against
 *
 * Not finding the Hub is never an error here. A vendor who cloned only this repository has one
 * tree and that is a correct checkout, so each caller falls back to a degraded mode — pins rather
 * than a comparison, an unmeasured figure rather than a wrong one. The danger is precisely that
 * the degraded mode is **silent and passing**, so a tool that cannot find a checkout which is
 * sitting beside it looks identical to a tool doing its job. A missing spelling therefore does not
 * fail anything; it removes a check and says nothing. One list is what keeps that from happening a
 * directory rename at a time.
 */

import { existsSync } from 'node:fs'
import { join, resolve } from 'node:path'

/**
 * Every directory name the Hub has been cloned under, most current first.
 *
 * The repository is `robotoverflow` on GitHub, "Robot Overflow" in prose, and whatever `git clone`
 * produced on disk; it was renamed from BotMarket, and a checkout made before the rename is still
 * a valid checkout. A case-sensitive filesystem makes every capitalisation a different directory,
 * which is why the variants are enumerated rather than matched loosely — a fuzzy match here would
 * eventually find some *other* repository beside this one and compare against it.
 */
export const HUB_DIRECTORY_NAMES = [
  'Robot Overflow',
  'robot-overflow',
  'RobotOverflow',
  'robotoverflow',
  'BotMarket',
  'botmarket',
  'Botmarket',
  'botMarket',
]

/**
 * The first of those that exists beside `root`, or `null` when none does.
 *
 * `null` rather than a guessed path, so a caller states its own degraded mode in its own words
 * instead of inheriting a fabricated one. `findHubOr` is for the callers that want a path to print.
 *
 * @param {string} root absolute path of this repository's checkout
 * @returns {string | null}
 */
export function findHub(root) {
  for (const name of HUB_DIRECTORY_NAMES) {
    const candidate = resolve(join(root, '..', name))
    if (existsSync(candidate)) return candidate
  }
  return null
}

/**
 * `findHub`, with the path a caller should name in a "not found" message when there is none.
 *
 * The fallback is the **current** spelling rather than the historical one: a message telling
 * somebody to check `../BotMarket` sends them looking for a directory that has not been produced
 * by a clone in months.
 *
 * @param {string} root absolute path of this repository's checkout
 * @returns {{ path: string, found: boolean }}
 */
export function findHubOr(root) {
  const found = findHub(root)
  if (found !== null) return { path: found, found: true }
  return { path: resolve(join(root, '..', 'RobotOverflow')), found: false }
}
