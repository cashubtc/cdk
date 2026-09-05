import { PageHead, Reveal, Callout } from '../primitives.tsx'
import { Lifecycle } from '../figures/lifecycle.tsx'

export default function LifecyclePage() {
  return (
    <>
      <PageHead
        idx="07"
        section="one proof"
        title={
          <>
            A v3 proof is <span className="cd">born and checked</span>
          </>
        }
        lead={
          <>
            The full lifecycle of one v3 proof. Nothing here is federation yet — but every step survives it,
            which is exactly the point.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Lifecycle />
      </Reveal>
      <Reveal delay={0.2}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">wallet prepares</h3>
            <ul className="col-list">
              <li>choose the mint's active keyset; if v3, all crypto is BLS</li>
              <li>derive <span className="math">secret</span> and blinding <span className="math">r</span> from seed</li>
              <li>blind <span className="math">B = r · H(secret)</span> in G1, send ordinary <span className="math">BlindedMessage</span>s</li>
              <li>HTTP/WS protocol unchanged</li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">wallet checks</h3>
            <ul className="col-list">
              <li>two-stage: blind pairing first, then unblind, then proof pairing</li>
              <li><span className="math">e(C_,G2) == e(B,K)</span> before trusting the response</li>
              <li>reject any v3 signature still carrying DLEQ</li>
            </ul>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.26}>
        <Callout tone="rose" k="the fix">
          early wallet code could construct a v3 proof and only check the unblinded pairing — or skip the blind check
          entirely. commit <span className="math">289fe9e44</span> wired{' '}
          <span className="math">verify_bls_blind_signature()</span> into mint-response validation, restore, auth-wallet
          mint, and token signature verification. <span className="math">verify_token_dleq</span> stays as a deprecated alias.
        </Callout>
      </Reveal>
      <Reveal delay={0.3}>
        <p className="note" style={{ marginTop: '1em' }}>
          tokens: v3 proofs are still ordinary Cashu proofs — no new token version. two serialization details matter:
          <span className="math">ProofV4.dleq</span> is now <span className="math">skip_serializing_if</span> absent,
          and a test proves a v3 token does not encode an empty DLEQ.
        </p>
      </Reveal>
    </>
  )
}