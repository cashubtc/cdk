import { PageHead, Reveal, Callout } from '../primitives.tsx'
import { Race } from '../figures/race.tsx'

export default function Consensus() {
  return (
    <>
      <PageHead
        idx="12"
        section="the ordering layer"
        title={
          <>
            Consensus <span className="cd">before</span> signing
          </>
        }
        lead={
          <>
            Threshold BLS makes any <span className="math">t</span> valid shares enough to finish a proof. Consensus
            is what puts the members back on one output set before anyone signs. Treat AlephBFT as a black box that
            emits one total order.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Race />
      </Reveal>
      <Reveal delay={0.22}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">a swap is a named envelope</h3>
            <p className="note" style={{ margin: 0 }}>
              not “please sign these outputs” —{' '}
              <span className="math">{'{federation, kind: Swap, inputs, outputs}'}</span>, and{' '}
              <span className="math">operation_id = hash(envelope)</span>. the id is the name of the proposed
              transaction. apply does the cashu part: verify, mark spent, sign the accepted outputs. replay is
              idempotent.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">mint vs swap lock</h3>
            <p className="note" style={{ margin: 0 }}>
              mint / melt conflict on the <b>quote id</b>. swap has no quote — the spent input proofs become the lock
              once the first accepted swap spends them. second window cannot collect shares. DLEQ is still absent; the
              pairing check is what makes shares cheap to verify.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.28}>
        <Callout k="why T can be below C">
          a 3-of-5 signing threshold with 4-of-5 BFT is safe only because shares are produced{' '}
          <em>after</em> the exact operation is accepted. if members signed first and ordered later, the lower signing
          threshold brings mix-and-match straight back. consensus is the agreement; BLS is only the compact way to emit
          the already-agreed signature.
        </Callout>
      </Reveal>
    </>
  )
}