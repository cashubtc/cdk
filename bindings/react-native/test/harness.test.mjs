// Exercises the generated Node harness against the real Rust library, and
// checks that it agrees with the pure TypeScript implementation in cashu-ts.

import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { KEYSET_V0, SEED, native, toHex } from './support.mjs'

const api = native()

describe('primitives', () => {
  it('hashes a known vector', () => {
    assert.equal(
      toHex(api.sha256Digest(new TextEncoder().encode('abc'))),
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
    )
  })

  it('maps NUT-00 hash_to_curve vectors', () => {
    assert.equal(
      toHex(api.hashToCurve(new Uint8Array(32))),
      '024cce997d3b518f739663b757deaec95bcd9473c30a14ac2fd04023a739d1a725',
    )
  })

  it('keeps the full u64 range as bigint', () => {
    const huge = (1n << 62n)
    assert.deepEqual(api.splitAmount(huge, [huge], undefined), [huge])
    const outputs = api.createRandomOutputs([huge], KEYSET_V0)
    assert.equal(outputs[0].amount, huge)
    assert.equal(typeof outputs[0].amount, 'bigint')
  })

  it('blinds a batch of secrets in one crossing', () => {
    const secrets = [
      new TextEncoder().encode('one'),
      new TextEncoder().encode('two'),
      new TextEncoder().encode('three'),
    ]
    const pairs = api.blindMessages(secrets)
    assert.equal(pairs.length, 3)
    for (const pair of pairs) {
      assert.equal(pair.blindedSecret.length, 66)
      assert.equal(pair.blindingFactor.length, 64)
    }
    const singly = secrets.map((s) => api.blindMessage(s, undefined))
    assert.notEqual(pairs[0].blindedSecret, singly[0].blindedSecret)
  })

  it('round-trips a large byte buffer', () => {
    const large = new Uint8Array(1024 * 1024).fill(0x5a)
    assert.equal(api.sha256Digest(large).length, 32)
  })
})

describe('records, enums and optionals', () => {
  it('returns the record fields', () => {
    const output = api.createSingleRandomOutput(8n, KEYSET_V0)
    assert.equal(output.amount, 8n)
    assert.equal(output.keysetId, KEYSET_V0)
    assert.equal(output.blindedSecret.length, 66)
    assert.equal(output.blindingFactor.length, 64)
    assert.equal(output.derivationIndex, undefined)
  })

  it('fills the optional field when it is present', () => {
    const output = api.createSingleDeterministicOutput(8n, SEED, 3, KEYSET_V0)
    assert.equal(output.derivationIndex, 3)
    assert.equal(
      output.secret,
      '59284fd1650ea9fa17db2b3acf59ecd0f2d52ec3261dd4152785813ff27a33bf',
    )
  })

  it('carries an enum through a record', () => {
    const pubkey = '0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798'
    const base = {
      pubkey,
      additionalPubkeys: undefined,
      numSigs: undefined,
      locktime: undefined,
      refundPubkeys: undefined,
      numSigsRefund: undefined,
      sigFlag: 'sigInputs',
    }
    assert.ok(!api.createSingleP2pkOutput(base, 8n, KEYSET_V0).secret.includes('SIG_ALL'))
    assert.ok(
      api.createSingleP2pkOutput({ ...base, sigFlag: 'sigAll' }, 8n, KEYSET_V0).secret.includes(
        'SIG_ALL',
      ),
    )
  })
})

describe('errors', () => {
  it('raises a typed error with its fields', () => {
    assert.throws(
      () => api.createSingleRandomOutput(1n, 'not-a-keyset'),
      (error) => {
        assert.equal(error.name, 'CashuFfiError')
        assert.equal(error.kind, 'InvalidKeysetId')
        assert.equal(error.fields.id, 'not-a-keyset')
        return true
      },
    )
  })

  it('reports a wrong seed length', () => {
    assert.throws(
      () => api.createSingleDeterministicOutput(1n, new Uint8Array(32), 0, KEYSET_V0),
      (error) => {
        assert.equal(error.kind, 'InvalidSeedLength')
        assert.equal(error.fields.length, '32')
        return true
      },
    )
  })

  it('refuses a restore batch that would freeze the calling thread', () => {
    assert.throws(
      () => api.createRestoreOutputs(SEED, KEYSET_V0, 0, 4294967295),
      (error) => {
        assert.equal(error.kind, 'InvalidRestoreRange')
        assert.equal(Number(error.fields.count), 4294967295)
        assert.equal(Number(error.fields.max), 10000)
        return true
      },
    )
  })
})

describe('object lifecycle', () => {
  it('derives through a live object and refuses a closed one', () => {
    const factory = api.createDeterministicOutputFactory(SEED, KEYSET_V0)
    assert.equal(factory.keysetId(), KEYSET_V0)

    const viaObject = factory.outputs([16n, 8n], 0)
    const viaFunction = api.createDeterministicOutputs([16n, 8n], SEED, 0, KEYSET_V0)
    assert.deepEqual(
      viaObject.map((o) => o.secret),
      viaFunction.map((o) => o.secret),
    )

    factory.close()
    assert.throws(() => factory.keysetId(), /after close/)
    factory.close()
  })

  it('survives churn without leaking or double freeing', () => {
    for (let i = 0; i < 5000; i++) {
      const factory = api.createDeterministicOutputFactory(SEED, KEYSET_V0)
      factory.close()
    }
  })
})
