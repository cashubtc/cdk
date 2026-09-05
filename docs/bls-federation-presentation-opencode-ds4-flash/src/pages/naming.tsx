import { PageHead, Reveal, Callout, Tag, Chip } from '../primitives.tsx'

export default function Naming() {
  return (
    <>
      <PageHead
        idx="01"
        section="say it first"
        title={<>One version, <span className="cd">three names</span></>}
        lead="The single most confusing thing in this talk is not the math. It is that the same keyset version has one name in conversation, another in the enum, and another in the first byte of its ID."
      />
      <Reveal delay={0.1}>
        <table className="atable">
          <thead>
            <tr>
              <th>user-facing</th>
              <th>rust enum</th>
              <th>keyset id prefix</th>
              <th>meaning</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td><Tag c="cs">v1</Tag></td>
              <td><span className="mono">KeySetVersion::Version00</span></td>
              <td><span className="mono">00</span> + 7 bytes</td>
              <td>original secp256k1 keyset IDs</td>
            </tr>
            <tr>
              <td><Tag c="cs">v2</Tag></td>
              <td><span className="mono">KeySetVersion::Version01</span></td>
              <td><span className="mono">01</span> + 32 bytes</td>
              <td>secp keysets committing to unit / fee / expiry</td>
            </tr>
            <tr>
              <td><Tag c="c1">v3</Tag></td>
              <td><span className="mono">KeySetVersion::Version02</span></td>
              <td><span className="mono">02</span> + 32 bytes</td>
              <td>BLS12-381 keysets</td>
            </tr>
          </tbody>
        </table>
      </Reveal>
      <Reveal delay={0.16}>
        <Callout tone="rose" k="second collision">
          Cashu already ships <b>TokenV3</b> / <b>ProofV3</b> — those are token encoding versions, not BLS keyset v3.
          “v3” in a conversation about federation always means the BLS keyset version.
        </Callout>
      </Reveal>
      <Reveal delay={0.22}>
        <p className="note" style={{ marginTop: '1.2em' }}>
          keep the table on the wall of the room: we say <Chip c="c1">BLS v3</Chip>, the enum says
          <Chip>Version02</Chip>, and the first hex byte of a v3 keyset ID is <span className="mono" style={{ color: 'var(--g1)' }}>02</span>.
        </p>
      </Reveal>
    </>
  )
}