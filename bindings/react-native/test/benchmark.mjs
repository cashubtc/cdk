// cashu-ts versus the Rust implementation, in one Node process.
//
// The native side goes through koffi rather than JSI, so the per-call overhead
// is not identical to React Native's. Everything else, the UniFFI buffer
// encoding included, is the same work the Nitro path does.

import { OutputData, splitAmount } from '@cashu/cashu-ts'
import { sha256 } from '@noble/hashes/sha2.js'

import { KEYSET_V0, SEED, native } from './support.mjs'

const api = native('release')

const KEYS = (() => {
  const keys = {}
  for (let i = 0; i < 32; i++) {
    keys[(1n << BigInt(i)).toString()] = '02' + Buffer.alloc(32, i + 1).toString('hex')
  }
  return { id: KEYSET_V0, keys }
})()

function measure(run, budgetMs = 1500) {
  run()
  let iterations = 0
  const started = process.hrtime.bigint()
  let elapsed = 0n
  while (elapsed < BigInt(budgetMs) * 1_000_000n) {
    run()
    iterations++
    elapsed = process.hrtime.bigint() - started
  }
  return Number(elapsed) / 1e6 / iterations
}

function row(label, outputs, typescript, rust) {
  const speedup = typescript / rust
  console.log(
    `${label.padEnd(30)} ${String(outputs).padStart(6)} ` +
      `${typescript.toFixed(3).padStart(11)} ms ${rust.toFixed(3).padStart(11)} ms ` +
      `${speedup.toFixed(1).padStart(7)}x ` +
      `${((typescript / outputs) * 1000).toFixed(1).padStart(9)} us ` +
      `${((rust / outputs) * 1000).toFixed(1).padStart(9)} us`,
  )
}

console.log('\ndeterministic output creation (NUT-13, v00 keyset: BIP32 hardened derivation)')
console.log(
  'case'.padEnd(30) +
    'count'.padStart(7) +
    'typescript'.padStart(15) +
    'rust'.padStart(15) +
    'speedup'.padStart(8) +
    'ts/output'.padStart(12) +
    'rust/output'.padStart(12),
)

const cases = [
  ['small: 1 sat', 1],
  ['medium: 1023 sats', 1023],
  ['large: 2^20-1 sats', 1048575],
]

for (const [label, amount] of cases) {
  const amounts = splitAmount(amount, KEYS.keys).map((a) => a.toBigInt())
  const typescript = measure(() => OutputData.createDeterministicData(amount, SEED, 0, KEYS))
  const rust = measure(() => api.createDeterministicOutputs(amounts, SEED, 0, KEYSET_V0))
  row(label, amounts.length, typescript, rust)
}

console.log('\nNUT-09 restore batches (blank outputs, the heaviest wallet workload)')
console.log(
  'case'.padEnd(30) +
    'count'.padStart(7) +
    'typescript'.padStart(15) +
    'rust'.padStart(15) +
    'speedup'.padStart(8) +
    'ts/output'.padStart(12) +
    'rust/output'.padStart(12),
)

for (const count of [10, 100, 500]) {
  const typescript = measure(() => {
    const out = []
    for (let i = 0; i < count; i++) {
      out.push(OutputData.createSingleDeterministicData(0, SEED, i, KEYSET_V0))
    }
    return out
  })
  const rust = measure(() => api.createRestoreOutputs(SEED, KEYSET_V0, 0, count))
  row(`restore ${count} counters`, count, typescript, rust)
}

console.log('\nrandom output creation (no BIP32, blinding only)')
console.log(
  'case'.padEnd(30) +
    'count'.padStart(7) +
    'typescript'.padStart(15) +
    'rust'.padStart(15) +
    'speedup'.padStart(8) +
    'ts/output'.padStart(12) +
    'rust/output'.padStart(12),
)

for (const [label, amount] of cases) {
  const amounts = splitAmount(amount, KEYS.keys).map((a) => a.toBigInt())
  const typescript = measure(() => OutputData.createRandomData(amount, KEYS))
  const rust = measure(() => api.createRandomOutputs(amounts, KEYSET_V0))
  row(label, amounts.length, typescript, rust)
}

console.log('\ncall overhead and serialization, where the native win disappears')
const tiny = new Uint8Array(32)
const oneMiB = new Uint8Array(1024 * 1024)
const nativeTiny = measure(() => api.sha256Digest(tiny), 500)
const jsTiny = measure(() => sha256(tiny), 500)
const nativeLarge = measure(() => api.sha256Digest(oneMiB), 500)
const jsLarge = measure(() => sha256(oneMiB), 500)
row('sha256 of 32 bytes', 1, jsTiny, nativeTiny)
row('sha256 of 1 MiB', 1, jsLarge, nativeLarge)
console.log(
  '\n  A 32 byte hash costs less than the round trip, so going native loses.\n' +
    '  A 1 MiB hash is dominated by copying the buffer in and out, so the win is\n' +
    '  small. Batch the work behind one call, as the output builders above do.',
)
console.log()
