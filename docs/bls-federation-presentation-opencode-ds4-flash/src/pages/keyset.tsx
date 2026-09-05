import { PageHead, Reveal, Callout } from '../primitives.tsx'

export default function Keyset() {
  return (
    <>
      <PageHead
        idx="04"
        section="the protocol switch"
        title={
          <>
            Keyset v3 <span className="cd">is</span> the switch
          </>
        }
        lead={
          <>
            No new token format. A third keyset version is added, and DHKE, derivation, signing and verification all
            branch on it. v3 keyset IDs are v2-style commitment IDs — the version byte rewritten and the pubkeys
            swapped for BLS G2 keys.
          </>
        }
      />
      <Reveal delay={0.1}>
        <table className="atable">
          <thead>
            <tr>
              <th>v</th>
              <th>enum</th>
              <th>id</th>
              <th>curve</th>
              <th>blind</th>
              <th>unblind</th>
              <th>verify</th>
              <th>dleq</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td><span className="tag cs">v1</span></td>
              <td className="mono">Version00</td>
              <td className="mono">00 + 7 bytes</td>
              <td>secp256k1</td>
              <td><span className="math">Y + rG</span></td>
              <td><span className="math">C_ − rK</span></td>
              <td>secp multiply / DLEQ</td>
              <td className="cd">optional</td>
            </tr>
            <tr>
              <td><span className="tag cs">v2</span></td>
              <td className="mono">Version01</td>
              <td className="mono">01 + 32 bytes</td>
              <td>secp256k1</td>
              <td><span className="math">Y + rG</span></td>
              <td><span className="math">C_ − rK</span></td>
              <td>secp multiply / DLEQ</td>
              <td className="cd">optional</td>
            </tr>
            <tr>
              <td><span className="tag c1">v3</span></td>
              <td className="mono">Version02</td>
              <td className="mono">02 + 32 bytes</td>
              <td>BLS12-381</td>
              <td><span className="math">r · H(secret)</span></td>
              <td><span className="math">r⁻¹ · C_</span></td>
              <td><span className="math">e(C, G2) == e(H, K)</span></td>
              <td className="cr">forbidden</td>
            </tr>
          </tbody>
        </table>
      </Reveal>
      <Reveal delay={0.16}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">v3 id recipe</h3>
<ol className="recipe-list">
              <li>sort public keys by amount</li>
              <li>concatenate <span className="math">amount:hex(pubkey)</span> entries</li>
              <li>append <span className="math">|unit:…</span>, optionally fee / expiry</li>
              <li>SHA-256 → 32 bytes, prefix version byte <span className="math">02</span></li>
            </ol>
            <p className="note" style={{ marginTop: '0.5em' }}>
              same width as v2 (33 bytes); pubkey bytes are compressed G2 keys.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">mint policy</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">use_keyset_v2 = None</span> → new keysets are v3;{' '}
              <span className="math">Some(false)</span> forces v1, <span className="math">Some(true)</span> forces v2.
              live secp keysets are preserved until an operator rotates them explicitly.
            </p>
            <p className="note" style={{ marginTop: '0.5em' }}>
              rotation takes a version argument now:{' '}
              <span className="math">rotate_keyset_by_version</span>, and mint-cli{' '}
              <span className="math">rotate-next-keyset --keyset-version v3</span>.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.24}>
        <Callout k="proto off-by-one">
          the on-wire version byte and the gRPC enum disagree by one: proto 1/2/3 map onto Version00 / Version01 /
          Version02. original <span className="mono">bls</span> signatory only named V1/V2; the Rust enum already
          understood 3 as v3.
        </Callout>
      </Reveal>
    </>
  )
}