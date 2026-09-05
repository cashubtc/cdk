import { PageHead, Reveal, Callout, Quote } from '../primitives.tsx'

export default function Crash() {
  return (
    <>
      <PageHead
        idx="23"
        section="resilience"
        title={
          <>
            The book, the pen, <span className="cd">and the seat</span>
          </>
        }
        lead={
          <>
            A federation stays up if fewer than the BFT fault bound crash. Recovering <em>one</em> crashed member is a
            different problem: that seat still owns unique secrets. Peers can replay history to it — they cannot give
            it a new identity or new BLS/FROST shares.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Quote>
          Peers can give you the book. Only your backup can give you the pen. The book is the journal, checkpoints,
          certificates, and catch-up. The pen is identity, BLS shares, and the sealed FROST root.
        </Quote>
      </Reveal>
      <Reveal delay={0.16}>
        <table className="atable">
          <thead>
            <tr>
              <th>lost</th>
              <th>recoverable?</th>
              <th>how</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td className="mono">process crash, DB intact</td>
              <td className="ck">yes</td>
              <td>ordinary restart, suffix verify</td>
            </tr>
            <tr>
              <td className="mono">journal / DB after good snapshot</td>
              <td className="ck">yes</td>
              <td>restore snapshot + manifest + secrets, catch up the tail</td>
            </tr>
            <tr>
              <td className="mono">entire DB, secrets intact</td>
              <td className="ck">yes, slow</td>
              <td>empty DB + genesis catch-up pages from peers</td>
            </tr>
            <tr>
              <td className="mono">network partition / lag</td>
              <td className="ck">yes</td>
              <td>peer catch-up; fail closed until caught up</td>
            </tr>
            <tr>
              <td className="mono">one member offline</td>
              <td className="ck">yes</td>
              <td>remaining C / T keep going</td>
            </tr>
            <tr>
              <td className="mono">identity key or BLS shares</td>
              <td className="cr">no</td>
              <td>not reconstructible — new federation</td>
            </tr>
            <tr>
              <td className="mono">FROST sealed share / sealing key</td>
              <td className="cr">no</td>
              <td>cannot sign Bitcoin/Lightning as that seat</td>
            </tr>
            <tr>
              <td className="mono">another member's secrets</td>
              <td className="cr">never</td>
              <td>wrong seat — activation / PoP fail</td>
            </tr>
          </tbody>
        </table>
      </Reveal>
      <Reveal delay={0.22}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">the mechanisms</h3>
            <ul className="col-list">
              <li>deterministic journal replay — same operation_id, same state, same stored shares</li>
              <li>order digest + materialized-state digest — same journal, different SQL projection is not caught up</li>
              <li>catch-up certificates for a contiguous range; replay then verify both digests</li>
              <li>checkpoints authenticate; they do not ship the database</li>
              <li>fail-closed gates: no public submit, no FROST, no observations, no broadcast until ready</li>
              <li>persist-before-send / persist-before-broadcast everywhere</li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">the restore cookbook</h3>
            <ol className="recipe-list">
              <li>process died, disk fine → start mintd, verify suffix, maybe catch up</li>
              <li>disk lost, backup exists → restore DB + configs + sealed FROST + manifest, audit, catch up to tip</li>
              <li>disk lost, only secrets → empty DB, genesis page catch-up, same secrets</li>
              <li>secrets lost → stop. that seat is dead. new setup, new roster, new DKG</li>
            </ol>
            <p className="note" style={{ marginTop: '0.5em' }}>
              no BIP39 seed regenerates a federation member. standalone CDK can derive mint keys from a mnemonic; a
              member is a named roster seat.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.28}>
        <Callout k="the materialized-state audit latch">
          if an operator audit sees state ≠ digest, the member latches failed. restart, peer agreement, and checkpoint
          promotion do not clear it. preserve the bad DB, restore a known-good generation with the <em>same</em>{' '}
          secrets, audit again.
        </Callout>
      </Reveal>
    </>
  )
}