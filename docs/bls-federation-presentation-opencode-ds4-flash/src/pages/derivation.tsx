import { PageHead, Reveal, Callout } from '../primitives.tsx'

export default function Derivation() {
  return (
    <>
      <PageHead
        idx="06"
        section="determinism"
        title={
          <>
            Every key is <span className="cd">rejection-sampled</span>
          </>
        }
        lead={
          <>
            Restore, blank change outputs, and deterministic minting all follow the keyset curve. The v3 work is
            mostly “canonical scalar, or try again” — three derivation fixes in two weeks.
          </>
        }
      />
      <Reveal delay={0.1}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">wallet secrets — NUT-13</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">v1</span> keeps the old BIP32 path. <span className="math">v2/v3</span> use
              HMAC-SHA256 KDF <span className="math">Cashu_KDF_HMAC_SHA256 ‖ keyset_id ‖ u64_BE(counter) ‖ 0x00</span>.
              Blinding factors use the same KDF with suffix <span className="math">0x01</span> plus an attempt counter —
              parsed as secp for v2, canonical BLS scalar for v3.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">mint amount keys — NUT-02</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">v1/v2</span>: hardened BIP32 key. <span className="math">v3</span> evolved three
              times — SHA-256 the child then reduce (1), reduce the child bytes directly (2), rejection-sample a second
              hardened index until the 32 bytes are a canonical non-zero scalar (3, current). The unit test proves the
              live key is the two-level rejection-sampled child.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.16}>
        <pre className="codeblock">{`# v3 blinding factor — Secret::from_seed, suffix 0x01
message = "Cashu_KDF_HMAC_SHA256" || keyset_id || u64_BE(counter) || 0x01

for attempt in 0..=u32::MAX:
    h = HMAC-SHA256(seed, message || u32_BE(attempt))
    reject all-zero
    accept if h is a canonical BLS scalar          # uniform over Fr*, never reduced raw bytes

# v3 mint amount key — MintKeySet::generate, per denomination
for amount index i:
    attempt = 0
    loop:
        child = BIP32(i' / attempt')              # two-level hardened path
        reject all-zero
        accept SecretKey::bls_from_slice(child)  # same Fr* rejection sampler
`}</pre>
        <p className="note" style={{ marginTop: '0.6em' }}>
          random ephemeral blinding uses <span className="math">SecretKey::generate_bls()</span> — the same sampler
          over <span className="math">OsRng</span>.
        </p>
      </Reveal>
      <Reveal delay={0.24}>
        <Callout k="the pattern">
          raw 32-byte strings are almost never a canonical BLS scalar. everything that enters{' '}
          <span className="math">Fr</span> goes through rejection sampling — blindings, mint keys, batch weights. two
          commits (<span className="math">1d71add6e</span>, <span className="math">59a7a80e5</span>) exist precisely
          because the first versions reduced instead.
        </Callout>
      </Reveal>
    </>
  )
}