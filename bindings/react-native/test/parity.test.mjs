// The native path must produce exactly what cashu-ts produces, otherwise a
// wallet that swaps implementations would derive secrets it cannot restore.

import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { OutputData, splitAmount } from '@cashu/cashu-ts'

import { KEYSET_V0, SEED, native } from './support.mjs'

const api = native()

/**
 * A keyset large enough to split any amount used here. Only the denominations
 * matter for output creation; the key values are used at `toProof` time.
 */
function keyset(count = 32) {
  const keys = {}
  for (let i = 0; i < count; i++) {
    keys[(1n << BigInt(i)).toString()] =
      '02' + Buffer.alloc(32, i + 1).toString('hex')
  }
  return { id: KEYSET_V0, keys }
}

const KEYS = keyset()

function nativeDeterministic(amount, counter) {
  const amounts = splitAmount(amount, KEYS.keys).map((a) => a.toBigInt())
  return api.createDeterministicOutputs(amounts, SEED, counter, KEYSET_V0)
}

describe('parity with cashu-ts', () => {
  it('derives the same deterministic outputs', () => {
    for (const amount of [1, 8, 31, 1023, 4096]) {
      const expected = OutputData.createDeterministicData(amount, SEED, 0, KEYS)
      const actual = nativeDeterministic(amount, 0)

      assert.equal(actual.length, expected.length, `count for ${amount}`)
      for (let i = 0; i < expected.length; i++) {
        assert.equal(actual[i].amount, expected[i].blindedMessage.amount.toBigInt())
        assert.equal(actual[i].blindedSecret, expected[i].blindedMessage.B_)
        assert.equal(
          actual[i].blindingFactor,
          expected[i].blindingFactor.toString(16).padStart(64, '0'),
        )
        assert.equal(actual[i].secret, new TextDecoder().decode(expected[i].secret))
      }
    }
  })

  it('derives the same output at an offset counter', () => {
    const expected = OutputData.createSingleDeterministicData(8, SEED, 4321, KEYSET_V0)
    const actual = api.createSingleDeterministicOutput(8n, SEED, 4321, KEYSET_V0)

    assert.equal(actual.blindedSecret, expected.blindedMessage.B_)
    assert.equal(actual.secret, new TextDecoder().decode(expected.secret))
  })

  it('blinds a caller-supplied secret the same way', () => {
    const secret = new TextEncoder().encode('a shared secret')
    const factor = new Uint8Array(32).fill(7)
    const pair = api.blindMessage(secret, factor)
    assert.equal(pair.blindingFactor, Buffer.from(factor).toString('hex'))
    assert.equal(pair.blindedSecret.length, 66)
  })
})
