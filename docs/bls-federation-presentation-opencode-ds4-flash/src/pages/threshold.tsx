import { PageHead, Reveal, Callout } from '../primitives.tsx'
import { Lagrange } from '../figures/lagrange.tsx'

export default function Threshold() {
  return (
    <>
      <PageHead
        idx="10"
        section="the scheme"
        title={
          <>
            Classic Shamir-threshold BLS, <span className="cd">applied to the blind</span>
          </>
        }
        lead={
          <>
            One secret mint key <span className="math">k</span> is split by a degree-<span className="math">(t−1)</span>{' '}
            polynomial. Any <span className="math">t</span> members reconstruct one ordinary v3 blind signature;
            fewer than <span className="math">t</span> learn nothing useful. Not bare multisig, not IETF aggregate-BLS.
          </>
        }
      />
      <Reveal delay={0.1}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">the share</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">f(x) = a₀ + a₁x + … + a₍ₜ₋₁₎x^(t−1)</span> with <span className="math">a₀ = k</span>.
              member <span className="math">i</span> holds <span className="math">k_i = f(i)</span>, and publishes{' '}
              <span className="math">K_i = k_i·G2</span>. the wallet still sees one <span className="math">K = k·G2</span>.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">sign + check</h3>
            <p className="note" style={{ margin: 0 }}>
              member <span className="math">i</span> signs the blinded point:{' '}
              <span className="math">C_i = k_i · B</span>. anyone can check{' '}
              <span className="math">e(C_i, G2) == e(B, K_i)</span> without knowing{' '}
              <span className="math">k_i</span>.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.16}>
        <Lagrange />
      </Reveal>
      <Reveal delay={0.24}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">combine in G1</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">λᵢ(0) = Πⱼ≠ᵢ (−j)/(i−j)</span>, then{' '}
              <span className="math">C = Σ λᵢ(0)·Cᵢ = f(0)·B = k·B</span>. the public shares interpolate the same way
              in G2: <span className="math">K = Σ λᵢ·Kᵢ</span>.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">then it is standalone v3</h3>
            <p className="note" style={{ margin: 0 }}>
              after aggregation: <span className="math">e(C,G2) == e(B,K)</span>, unblind{' '}
              <span className="math">proof = r⁻¹·C</span>, and check <span className="math">e(proof,G2) == e(H,K)</span>.
              a 1-of-1 keyset is exactly a single-signer BLS key — the tests assert that.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.3}>
        <Callout k="what this is not">
          not bare multisig (the token does not keep <span className="math">C₁…Cₜ</span>), not FROST (that is the
          later Bitcoin-custody scheme on secp256k1), not “BLS aggregation” of different messages — every share here
          is a share of the <em>same</em> <span className="math">k·B</span>. because combination happens on the blind,
          members never see the secret, and the rest of cashu stays dumb.
        </Callout>
      </Reveal>
    </>
  )
}