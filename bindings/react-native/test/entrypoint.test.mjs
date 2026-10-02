// The root entrypoint must stay importable by a consumer that never installed
// cashu-ts. Only the `/creator` subpath may reach it, so this walks the built
// module graph rather than trusting the import list by eye.

import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it } from 'node:test'

const here = dirname(fileURLToPath(import.meta.url))
const lib = join(here, '../lib')

const manifest = JSON.parse(readFileSync(join(here, '../package.json'), 'utf8'))

/** Comments are dropped so an `@example` in a doc block is not read as an import. */
function stripComments(source) {
  return source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')
}

function specifiersIn(source) {
  const found = []
  const pattern = /(?:\bfrom|\bimport)\s*\(?\s*['"]([^'"]+)['"]/g
  let match
  while ((match = pattern.exec(source)) !== null) found.push(match[1])
  return found
}

/**
 * tsc emits extensionless relative imports under `moduleResolution: bundler`,
 * so each candidate spelling has to be tried.
 */
function resolveRelative(from, specifier) {
  const base = resolve(dirname(from), specifier)
  for (const candidate of [base, `${base}.js`, join(base, 'index.js')]) {
    if (candidate.endsWith('.js') && existsSync(candidate)) return candidate
  }
  throw new Error(`cannot resolve ${specifier} from ${from}`)
}

/** Every bare package the module graph rooted at `entry` pulls in. */
function packagesReachedFrom(entry) {
  assert.ok(
    existsSync(entry),
    `${entry} is missing: build the package first with "npm run build"`,
  )
  const seen = new Set()
  const packages = new Set()
  const queue = [entry]
  while (queue.length > 0) {
    const file = queue.pop()
    if (seen.has(file)) continue
    seen.add(file)
    for (const specifier of specifiersIn(stripComments(readFileSync(file, 'utf8')))) {
      if (specifier.startsWith('.')) queue.push(resolveRelative(file, specifier))
      else packages.add(specifier)
    }
  }
  return packages
}

describe('entrypoints', () => {
  it('the root reaches the native module without reaching cashu-ts', () => {
    const packages = packagesReachedFrom(join(lib, 'index.js'))
    assert.ok(
      !packages.has('@cashu/cashu-ts'),
      `the root entrypoint must not import cashu-ts, but reaches ${[...packages].join(', ')}`,
    )
    assert.deepEqual([...packages].sort(), ['react-native-nitro-modules'])
  })

  it('the creator subpath is the one that reaches cashu-ts', () => {
    const packages = packagesReachedFrom(join(lib, 'NativeOutputDataCreator.js'))
    assert.ok(packages.has('@cashu/cashu-ts'))
  })

  it('declares the creator subpath and cashu-ts as an optional peer', () => {
    assert.equal(manifest.exports['./creator'].default, './lib/NativeOutputDataCreator.js')
    assert.equal(manifest.peerDependencies['@cashu/cashu-ts'], '^5.0.0-rc.8')
    assert.equal(manifest.peerDependenciesMeta['@cashu/cashu-ts'].optional, true)
  })
})
