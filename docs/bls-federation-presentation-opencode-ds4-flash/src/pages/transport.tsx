import { PageHead, Reveal, Callout } from '../primitives.tsx'
import { Transport } from '../figures/transport.tsx'

export default function TransportPage() {
  return (
    <>
      <PageHead
        idx="13"
        section="data flow"
        title={
          <>
            Two planes, <span className="cd">one federation</span>
          </>
        }
        lead={
          <>
            There are two networks in a federation, not one. Same members, two jobs, two topologies: the public plane
            is wallet to mint; the private plane is member to member. HTTP and Iroh are just two ways to carry those
            same two planes.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Transport />
      </Reveal>
      <Reveal delay={0.2}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">fan-out semantics</h3>
            <ul className="col-list">
              <li>a star, not a gossip mesh — the wallet is the hub and the aggregator</li>
              <li>members do not send shares to each other; bad shares are dropped</li>
              <li>liveness = the signature threshold; a slow minority can be ignored</li>
              <li>two members returning shares for different output sets cannot be mixed</li>
              <li>code: <span className="math">crates/cdk/src/wallet/federation.rs</span></li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">the one exception: quote creation</h3>
            <ul className="col-list">
              <li>no need for <span className="math">t</span> invoice quotes — first healthy member, then threshold visibility</li>
              <li>status polls fan out again; never trust one member's “paid” or “issued”</li>
              <li>private plane is authenticated with the member identity key — wallets are not on it</li>
              <li>the token that leaves the wallet is still one ordinary Cashu proof</li>
            </ul>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.26}>
        <Callout k="fan-out is not enough">
          if M1 and M4 only ever hear from the wallet, they can still be shown different windows. the mesh is what
          lets them agree <em>before</em> they sign — and later runs the DKG ceremony.
        </Callout>
      </Reveal>
    </>
  )
}