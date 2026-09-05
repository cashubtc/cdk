import { PageHead, Reveal, Tag } from '../primitives.tsx'

export default function Mixed() {
  return (
    <>
      <PageHead
        idx="08"
        section="compatibility"
        title={
          <>
            Mixed-curve types are a <span className="cd">minefield</span>
          </>
        }
        lead={
          <>
            Once a <span className="math">PublicKey</span> can be secp, G1 or G2, old assumptions break — and some old
            secp helpers panic when handed a BLS point. The chapter title could be “we had to make the old SDK safe.”
          </>
        }
      />
      <Reveal delay={0.1}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">dispatch by length</h3>
            <ul className="col-list">
              <li><span className="math">Secp256k1</span> 33B / <span className="math">BlsG1</span> 48B / <span className="math">BlsG2</span> 96B</li>
              <li>identity &amp; infinity rejected at parse</li>
              <li>zero scalars rejected for blindings, mint keys, batch weights</li>
              <li>NUT-00 batch vector encoded as a unit test</li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">the panic surface</h3>
            <p className="note" style={{ margin: 0 }}>
              a compressed G1 point is valid hex and valid <span className="math">PublicKey::from_slice</span> input —
              so an attacker can place a BLS point in P2PK/HTLC spending data. Before the fix it panicked in{' '}
              <span className="math">x_only_public_key()</span>. After: <span className="math">NonSecp256k1Pubkey</span>.
            </p>
            <p className="note" style={{ marginTop: '0.5em' }}>
              P2PK / HTLC stay <Tag c="cs">secp-only</Tag>. Panic surface remains on: to_uncompressed_bytes,
              x_only_public_key, negate, to_scalar / as_scalar.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.16}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">fuzzing had to change</h3>
            <ul className="col-list">
              <li>random hex almost never decodes to a subgroup-correct BLS point</li>
              <li><span className="math">fuzz_bls_pubkey_in_conditions</span> injects <em>valid</em> compressed G1/G2 points into P2PK/HTLC fields, asserts error, not panic</li>
              <li><span className="math">fuzz_bls_dhke</span> — parse, blind/sign/unblind/verify, batch weights, soundness (honest proofs verify, tampering fails)</li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">keep these close</h3>
            <ul className="col-list">
              <li><span className="math">nut01/bls.rs</span> — threshold types already live here on <span className="mono">bls-federation</span></li>
              <li><span className="math">nut01/public_key.rs</span>, <span className="math">secret_key.rs</span>, <span className="math">mod.rs</span></li>
              <li><span className="math">nut02.rs</span>, <span className="math">nut00/mod.rs</span>, <span className="math">nut00/token.rs</span></li>
              <li>dhke.rs, nut11/mod.rs, nut12.rs, nut13.rs</li>
              <li>mint/builder.rs, keysets/mod.rs, signatory, mintd config</li>
            </ul>
          </div>
        </div>
      </Reveal>
    </>
  )
}