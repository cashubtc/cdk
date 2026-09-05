import { PageHead, Reveal, Quote } from '../primitives.tsx'

export default function Fork() {
  return (
    <>
      <PageHead
        idx="09"
        section="design fork"
        title={
          <>
            Federate the <span className="cd">issuer</span>, not the <span className="cd">token</span>
          </>
        }
        lead={
          <>
            “We already had Chaumian signatures — why not t-of-n mint signatures, like Bitcoin bare multisig?” It
            would have worked as a prototype. It would have been a worse Cashu federation. Keep the two secp
            alternatives apart: bare multisig ≠ threshold secp.
          </>
        }
      />
      <Reveal delay={0.1}>
        <table className="atable">
          <thead>
            <tr>
              <th></th>
              <th>bare secp multisig</th>
              <th>BLS threshold shares</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td className="cd">what the wallet stores</td>
              <td><span className="cr">t signatures + roster</span></td>
              <td>one <span className="math">C</span> under one <span className="math">K</span></td>
            </tr>
            <tr>
              <td className="cd">who can receive the token</td>
              <td className="cr">federation-aware wallets</td>
              <td className="ck">any v3 cashu wallet</td>
            </tr>
            <tr>
              <td className="cd">verify</td>
              <td className="cr">t secp checks + policy</td>
              <td className="ck">one pairing, or a batch</td>
            </tr>
            <tr>
              <td className="cd">offline proof of mint key</td>
              <td className="cr">per-signer DLEQ</td>
              <td className="ck">pairing against advertised <span className="math">K</span></td>
            </tr>
            <tr>
              <td className="cd">keyset</td>
              <td className="cr">set of keys</td>
              <td className="ck">ordinary v3 keyset</td>
            </tr>
            <tr>
              <td className="cd">still need consensus?</td>
              <td>yes</td>
              <td>yes</td>
            </tr>
          </tbody>
        </table>
      </Reveal>
      <Reveal delay={0.18}>
        <Quote>
          Bitcoin can afford bare multisig because the coin lives in a UTXO that already names its script. Cashu
          cannot, because the coin <em>is</em> the signature.
        </Quote>
      </Reveal>
      <Reveal delay={0.24}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">why interpolated secp is still worse</h3>
            <p className="note" style={{ margin: 0 }}>
              The algebra interpolates fine. The hard part is proving the interpolated point is{' '}
              <span className="math">k · Y</span> for the advertised aggregate key — with no{' '}
              <span className="math">k</span>, DLEQ dies. Per-share DLEQ + hope, a new threshold-DLEQ protocol, or
              trusting the shares. BLS runs the same check for a share and for the aggregate:
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">and the honest counterweight</h3>
            <p className="note" style={{ margin: 0 }}>
              Bare multisig still gets blindness, t-of-n availability, and a fast trusted 2-of-3 prototype right. The
              expensive federation work — consensus, replay, catch-up, payment observation — is not avoided by secp.
            </p>
          </div>
        </div>
      </Reveal>
    </>
  )
}