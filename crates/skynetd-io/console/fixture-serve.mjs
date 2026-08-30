/**
 * Serve a console fixture *with the status codes the robot actually answered*.
 *
 *     cargo test -p skynetd-io -- --ignored console_fixture --nocapture
 *     node <fixture>/fixture-serve.mjs 8080      # then open http://127.0.0.1:8080/
 *
 * # Why this exists rather than "any static server will do"
 *
 * The fixture writes one file per route and one `<route>.status` beside it, and the comment on the
 * test says why: *"a 503 replayed as a 200 renders an error object as data and hides the behaviour
 * worth looking at."* A plain static server does exactly that — it has no idea the file next to
 * `blackbox` is a status code — so the page it renders shows `undefined records of undefined B`
 * where the robot said **service unavailable**, which is the precise §32.2 violation the console is
 * built not to commit.
 *
 * That is not hypothetical: it is what a static server drew the first time somebody looked. The
 * fixture was honest and the way it was being served was not, so the fix belongs here.
 *
 * Node's standard library only. This runs on a laptop beside a robot, and a fixture that needed an
 * `npm install` would be a fixture nobody opens.
 */

import { createServer } from 'node:http'
import { existsSync, readFileSync } from 'node:fs'
import { dirname, extname, join, normalize } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const PORT = Number(process.argv[2] || 8080)

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.png': 'image/png',
}

createServer((request, response) => {
  const path = (request.url || '/').split('?')[0]
  const name = path === '/' ? 'index.html' : decodeURIComponent(path.slice(1))
  const file = join(HERE, normalize(name))

  // A fixture is a local toy and still not a directory traversal: `normalize` collapses `..`, and
  // this rejects anything that climbed out.
  if (!file.startsWith(HERE) || !existsSync(file) || name.endsWith('.status')) {
    response.writeHead(404, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ error: 'no such route', detail: `nothing recorded for ${path}` }))
    return
  }

  const beside = `${file}.status`
  const status = existsSync(beside) ? Number(readFileSync(beside, 'utf8').trim()) || 200 : 200

  response.writeHead(status, {
    'content-type': TYPES[extname(name)] || 'application/json',
    'cache-control': 'no-store',
  })
  response.end(readFileSync(file))
}).listen(PORT, '127.0.0.1', () => {
  console.log(`console fixture on http://127.0.0.1:${PORT}/`)
  console.log('each route answers with the status code the daemon gave it')
})
