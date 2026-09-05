import { Reveal, Legend } from '../primitives.tsx'
import { CoverGraph } from '../figures/covergraph.tsx'

export default function Cover() {
  return (
    <>
      <Reveal>
        <div className="eyebrow">
          <span className="idx">00</span>
          <span className="tick" />
          <span>CDK · federated cashu</span>
        </div>
      </Reveal>
      <Reveal delay={0.05}>
        <h1 className="cover-title">
          Federating the <span className="cover-acc">issuer</span>,
          <br />
          not the <span className="cover-dim">token</span>
        </h1>
      </Reveal>
      <Reveal delay={0.12}>
        <p className="cover-lead">
          From single-signer BLS12-381 keysets to threshold issuance — Shamir shares, AlephBFT consensus,
          dealer-free DKG, and a two-plane federation that still spends as one ordinary Cashu proof.
        </p>
      </Reveal>
      <Reveal delay={0.18}>
        <CoverGraph />
      </Reveal>
      <Reveal delay={0.26}>
        <div className="cover-legend">
          <Legend
            items={[
              ['g1dot', 'G1 — blinded messages & signatures'],
              ['g2dot', 'G2 — public keys'],
              ['secpdot', 'secp256k1 legacy'],
              ['rosec', 'attack / open hole'],
              ['okdot', 'accepted'],
            ]}
          />
          <span className="cover-src">content: docs/FEDERATION_NOTES.md</span>
        </div>
      </Reveal>
    </>
  )
}