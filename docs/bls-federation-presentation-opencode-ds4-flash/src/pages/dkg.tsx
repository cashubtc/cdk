import { PageHead, Reveal, Callout } from '../primitives.tsx'
import { Dkg } from '../figures/dkg.tsx'

export default function DkgPage() {
  return (
    <>
      <PageHead
        idx="14"
        section="dealer-free"
        title={
          <>
            BLS DKG — the secret <span className="cd">never exists</span>
          </>
        }
        lead={
          <>
            Dealer-free Pedersen DKG: every member contributes a polynomial, and the joint mint key is the sum of
            their constant terms. At the end the wallet still sees one ordinary v3 keyset. The ceremony runs on the
            private member plane — never on the wallet fan-out.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Dkg />
      </Reveal>
      <Reveal delay={0.22}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">what they are generating</h3>
            <p className="note" style={{ margin: 0 }}>
              threshold <span className="math">t</span>, one polynomial per denomination:{' '}
              <span className="math">f(x) = k + a₁x + … + a₍ₜ₋₁₎x^(t−1)</span>. member{' '}
              <span className="math">i</span> ends with <span className="math">k_i = Σⱼ fⱼ(i)</span> — exactly the
              Shamir share used later to sign <span className="math">C_i = k_i·B</span>. once per amount, because a
              keyset is one key per amount.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">the aggregate public key</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">K = C₁[0] + C₂[0] + … + Cₙ[0] = k·G2</span>. That{' '}
              <span className="math">K</span> goes into the v3 keyset. Wallets never see the dance — and each member
              publishes <span className="math">K_i = k_i·G2</span> so later signature shares can be checked without
              reconstructing <span className="math">k</span>.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.28}>
        <Callout k="why after fan-out and consensus">
          DKG uses the private mesh, not wallet fan-out. after DKG, issuance is still: wallet fans out, members agree
          on the exact operation, each uses its <span className="math">k_i</span>. code:{' '}
          <span className="math">cdk-common/src/federation/dkg.rs</span> + share helpers in{' '}
          <span className="math">nut01/bls.rs</span>.
        </Callout>
      </Reveal>
    </>
  )
}