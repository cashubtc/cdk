import { PageHead, Reveal, Callout } from '../primitives.tsx'

export default function Blocks() {
  return (
    <>
      <PageHead
        idx="05"
        section="crypto blocks"
        title={
          <>
            Types, curves, <span className="cd">and the branch on version</span>
          </>
        }
        lead={
          <>
            Once a <span className="math">PublicKey</span> can be secp, G1, or G2, every DHKE helper has to dispatch
            on the keyset version. These are the pieces the v3 switch stands on.
          </>
        }
      />
      <Reveal delay={0.1}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">key types</h3>
            <ul className="col-list">
              <li>
                <span className="math">Secp256k1</span> — 33-byte compressed, legacy
              </li>
              <li>
                <span className="math">BlsG1</span> — 48-byte compressed, blinded msgs &amp; signatures
              </li>
              <li>
                <span className="math">BlsG2</span> — 96-byte compressed, mint public keys
              </li>
              <li>parsing is by length; identity / infinity rejected</li>
              <li>BLS scalars big-endian on the wire, reversed for <span className="math">bls12_381::Scalar</span></li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">hash-to-curve</h3>
            <ul className="col-list">
              <li>secp: old loop, DST <span className="math">Secp256k1_HashToCurve_Cashu_</span></li>
              <li>v3: <span className="math">ExpandMsgXmd&lt;Sha256&gt;</span> with DST
                <span className="math">CASHU_BLS12_381_G1_XMD:SHA-256_SSWU_RO_</span></li>
              <li><span className="math">hash_to_curve_for_version()</span> is what <span className="math">Proof::y()</span> uses</li>
            </ul>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.16}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">dhke entry points</h3>
            <ul className="col-list">
              <li><span className="math">blind_message_for_version(secret, r, v)</span></li>
              <li><span className="math">construct_proofs</span> branches on <span className="math">keyset_id.get_version()</span></li>
              <li><span className="math">sign_message(k, B)</span> matches <span className="math">SecretKey::Secp256k1 | Bls</span></li>
              <li>verify set: <span className="math">verify_bls_message</span>, <span className="math">verify_bls_blind_signature</span>, <span className="math">verify_bls_message_keyed</span>, <span className="math">batch_verify_bls_messages</span></li>
              <li><span className="cr">v3 DLEQ is rejected</span> in <span className="math">construct_proofs</span> — NUT-12 returns <span className="math">None</span> for v3</li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">batch weights, per NUT-00</h3>
            <ol className="recipe-list">
              <li>transcript prefix <span className="math">Cashu_BLS_Batch_v1</span></li>
              <li>per item: <span className="math">C || K || u32_BE(len msg) || msg</span></li>
              <li><span className="math">challenge = SHA256(transcript)</span></li>
              <li>rejection-sample <span className="math">SHA256(challenge ‖ i ‖ ctr)</span> until a canonical non-zero <span className="math">Fr*</span></li>
            </ol>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.24}>
        <Callout k="the first batch impl failed the vector">
          bytes reduced into scalars with a one-byte counter — wrong. commit{' '}
          <span className="math">5d4d0d7ac</span> switched to rejection sampling to match the published NUT-00 vector.
          there is also a later federation-era <span className="math">Cashu_BLS_Blind_Batch_v1</span> constant.
        </Callout>
      </Reveal>
    </>
  )
}