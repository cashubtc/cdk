import { PageHead, Reveal, Callout } from '../primitives.tsx'
import { Duel } from '../figures/duel.tsx'

export default function WhyBls() {
  return (
    <>
      <PageHead
        idx="03"
        section="motivation"
        title={
          <>
            Why BLS, <span className="cd">before</span> federation
          </>
        }
        lead={
          <>
            Standalone Cashu v1/v2 works for one mint operator, but you cannot split{' '}
            <span className="math">k</span> across members and still let a wallet verify an ordinary proof against one
            public key. BLS12-381 swaps the algebra for a multiplicative pairing scheme so the secret can later be
            shared — and the wallet never notices.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Duel />
      </Reveal>
      <Reveal delay={0.2}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">verify the proof</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">e(C, G2) == e(H(secret), K)</span> — the blinded signature and the public key
              pair to the same value. no mint secret required.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">verify the blind signature</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">e(C_, G2) == e(B, K)</span> — checked by wallets <em>before</em> trusting the
              mint response. this is the check the v3 fix added.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.26}>
        <Callout tone="secp" k="scope">
          the original <span className="mono">bls</span> branch is still single-signer BLS. no threshold shares,
          no Lagrange interpolation, no DKG, no federation keyset gates, no rosters. those all start on{' '}
          <span className="mono">bls-federation</span>.
        </Callout>
      </Reveal>
    </>
  )
}