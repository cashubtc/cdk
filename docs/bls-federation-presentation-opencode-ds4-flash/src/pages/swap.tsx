import { PageHead, Reveal, Tag } from '../primitives.tsx'

export default function Swap() {
  return (
    <>
      <PageHead
        idx="20"
        section="ecash only"
        title={
          <>
            Swap, step by step — <span className="cd">no bitcoin involved</span>
          </>
        }
        lead={
          <>
            A federated swap never touches Bitcoin: no quote, no BDK, no FROST. Just eCash — spend old proofs, issue
            new ones — protected by the same consensus-before-sign rule as mint, without a funding backend.
          </>
        }
      />
      <Reveal delay={0.1}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">messages</h3>
            <ul className="col-list">
              <li>
                public: <span className="math">POST /swap</span> with a normal <span className="math">SwapRequest</span>{' '}
                — in federation mode the “signatures” are that member's BLS shares
              </li>
              <li>
                private: <span className="math">SubmitOperation</span> of a <span className="math">Swap</span> envelope,
                AlephBFT activity, peer status &amp; journal catch-up
              </li>
              <li>
                no address allocation, no chain observation, no tx proposal, no FROST rounds
              </li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">admission + apply</h3>
            <p className="note" style={{ margin: 0 }}>
              admission rejects empty inputs, duplicate inputs/outputs, non-v3 outputs, oversized proof/witness
              content, unbalanced amounts — before the envelope is proposed. apply: verify input proofs against the
              aggregate v3 keys (including P2PK/HTLC/SIG_ALL), verify outputs are v3 and the tx balances after fees,
              mark spent, persist blinded messages, produce shares{' '}
              <span className="math">C_i = k_i·B</span> bound to <span className="math">operation_id</span> + member +
              keyset + amount + output index.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.18}>
        <pre className="codeblock">{`wallet ── POST /swap (inputs P1,P2 → outputs A,B) ──► every member
    │                                                     │
    │          envelope = {federation, kind: Swap,        │
    │                     request: {inputs, witnesses,    │
    │                               outputs}}             │
    │          operation_id = hash(envelope)              │
    │                                                     ▼
    │                                           consensus box — S_AB wins
    │                                                     │
    ◄───────────── persisted_swap_share_response ──────────┘
    wallet needs T shares for the SAME outputs:
    e(C_i, G2) == e(B, K_i)  →  Lagrange → one C → v3 proof`}</pre>
      </Reveal>
      <Reveal delay={0.24}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">guarantees</h3>
            <ul className="col-list">
              <li>only one exact input/output set can be signed</li>
              <li>inputs spent once, across the roster</li>
              <li>shares exist only after accept, and only for accepted outputs</li>
              <li>restore returns those same shares, never new ones</li>
              <li>offline members can catch up and serve accepted shares — they cannot invent another window</li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">… and what it does not give</h3>
            <p className="note" style={{ margin: 0 }}>
              unless the proofs already use <Tag c="cr">SIG_ALL</Tag>: a Byzantine member that saw the bearer proofs
              can rewrite <span className="math">P1,P2 → X,Y</span> and race that envelope first. if SIG_ALL is
              present, mutating outputs changes the operation ID <em>and</em> fails apply — that is why SIG_ALL is the
              remaining client-intent hole, not a missing swap step.
            </p>
          </div>
        </div>
      </Reveal>
    </>
  )
}