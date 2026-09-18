// Shared setup for the Node harness tests and the benchmark.

import { existsSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { load } from './generated/CashuCrypto.koffi.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(here, '../../..')

/**
 * The Rust library. With no profile named, the most recently built one wins,
 * so a test run always sees the build that just happened.
 */
export function libraryPath(profile) {
  const found = []
  for (const name of profile ? [profile] : ['release', 'debug']) {
    for (const extension of ['dylib', 'so', 'dll']) {
      const path = join(repoRoot, 'target', name, `libcashu_ffi.${extension}`)
      if (existsSync(path)) found.push([statSync(path).mtimeMs, path])
    }
  }
  if (found.length === 0) {
    throw new Error('build the library first: cargo build -p cashu-ffi')
  }
  found.sort((a, b) => b[0] - a[0])
  return found[0][1]
}

export function native(profile) {
  return load(libraryPath(profile))
}

/** NUT-13 test-vector mnemonic seed, as 64 raw bytes. */
export const SEED = Uint8Array.from(
  Buffer.from(
    'dd44ee516b0647e80b488e8dcc56d736a148f15276bef588b37057476d4b2b25' +
      '780d3688a32b37353d6995997842c0fd8b412475c891c16310471fbc86dcbda8',
    'hex',
  ),
)

export const KEYSET_V0 = '009a1f293253e41e'

/** A keyset whose keys are real points, so cashu-ts will accept it. */
export function keysetFor(secp, count = 32) {
  const keys = {}
  for (let i = 0; i < count; i++) {
    keys[(1n << BigInt(i)).toString()] = secp[i % secp.length]
  }
  return { id: KEYSET_V0, keys }
}

export function toHex(bytes) {
  return Buffer.from(bytes).toString('hex')
}
