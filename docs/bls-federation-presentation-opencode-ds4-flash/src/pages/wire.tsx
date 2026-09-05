import { useState } from 'react'
import { PageHead, Reveal } from '../primitives.tsx'
import { Ledger, type LedgerStep } from '../figures/ledger.tsx'

type FlowDef = { id: string; label: string; steps: LedgerStep[]; out: string }

const FLOWS: FlowDef[] = [
  {
    id: 'swap',
    label: 'swap',
    out: 'T BLS shares (off-log)',
    steps: [{ label: 'Swap', count: '1×', tone: 'g1', note: 'one winner' }],
  },
  {
    id: 'mint-fw',
    label: 'mint · fakewallet',
    out: 'BLS shares → v3 proof',
    steps: [
      { label: 'MintQuote', count: '1×', tone: 'g2' },
      { label: 'MintQuotePayment', count: '1×', tone: 'ok', note: 'one observer' },
      { label: 'Mint', count: '1×', tone: 'g1' },
    ],
  },
  {
    id: 'mint-bdk',
    label: 'mint · federated BDK',
    out: 'BLS shares → v3 proof',
    steps: [
      { label: 'MintQuote', count: '1×', tone: 'g2' },
      { label: 'AddressAllocation', count: '1×', tone: 'g2' },
      { label: 'ChainObservation::Deposit', count: 'D×', tone: 'secp', note: 'D is usually n or Q' },
      { label: 'MintQuotePayment', count: 'Q×', tone: 'ok' },
      { label: 'Mint', count: '1×', tone: 'g1' },
    ],
  },
  {
    id: 'melt-fw',
    label: 'melt · fakewallet',
    out: 'paid, maybe BLS change',
    steps: [
      { label: 'MeltQuote', count: '1×', tone: 'g2' },
      { label: 'Melt', count: '1×', tone: 'g1', note: 'spend proofs, maybe change' },
      { label: 'MeltQuotePayment', count: '1×', tone: 'ok', note: 'one observer' },
    ],
  },
  {
    id: 'melt-bdk',
    label: 'melt · federated BDK',
    out: 'bitcoin moves',
    steps: [
      { label: 'MeltQuote', count: '1×', tone: 'g2' },
      { label: 'Melt', count: '1×', tone: 'g1', note: 'payment intent, no coins' },
      { label: 'AddressAllocation', count: '0–1×', tone: 'g2', note: 'internal change' },
      { label: 'TransactionProposal', count: '1×', tone: 'secp' },
      { label: 'TransactionSigned', count: '1×', tone: 'secp' },
      { label: 'BroadcastIntent', count: '1×', tone: 'secp' },
      { label: 'BroadcastObservation', count: 'B×', tone: 'ok' },
      { label: 'MeltQuotePayment', count: 'Q×', tone: 'ok' },
    ],
  },
]

export default function Wire() {
  const [flow, setFlow] = useState(FLOWS[2])

  return (
    <>
      <PageHead
        idx="21"
        section="on the wire"
        title={
          <>
            What AlephBFT actually <span className="cd">orders</span>
          </>
        }
        lead={
          <>
            AlephBFT does not order raw HTTP. it orders <span className="math">FederationConsensusItem</span>s —
            almost every cash-relevant item is one wrapped envelope whose hash is the{' '}
            <span className="math">operation_id</span>. wallet fan-out, DKG, FROST rounds, peer status and catch-up
            ride the private REST plane instead.
          </>
        }
      />
      <Reveal delay={0.1}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">wallet value operations</h3>
            <ul className="col-list">
              <li>MintQuote / MintQuotePayment is submitted by the first healthy member</li>
              <li>Mint — paid quote(s) + exact blinded outputs; conflict = quote id</li>
              <li>MeltQuote / Melt — quote + proofs + optional change; conflict = quote id</li>
              <li>Swap — inputs + witnesses + outputs; lock = spent proofs at apply</li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">treasury & custody</h3>
            <ul className="col-list">
              <li>observations are one envelope <em>per observer</em>: MintQuote/MeltQuotePayment, Deposit/Reorg, BroadcastObservation</li>
              <li>on-chain: AddressAllocation, TransactionProposal, TransactionSigned (+ per-input FROST audit), BroadcastIntent</li>
              <li>housekeeping: KeysetRotation, RecoveryBarrier</li>
              <li>Bark: ReceiveIntent → ReceivePrepared → ActionPlan/ActionTransition → EvidenceCertificate</li>
            </ul>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.16}>
        <div className="lagrange-btns">
          {FLOWS.map((f) => (
            <button key={f.id} type="button" className={`btn ${flow.id === f.id ? 'on' : ''}`} onClick={() => setFlow(f)}>
              {f.label}
            </button>
          ))}
        </div>
        <Ledger key={flow.id} steps={flow.steps} flow={flow.out} />
      </Reveal>
      <Reveal delay={0.24}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">who has to show up</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">n</span> members: BFT needs{' '}
              <span className="math">C = n − ⌊(n−1)/3⌋</span> (5 members → 4), signing <span className="math">T ≤ C</span>.
              ordering a unique operation: <span className="math">C</span> members finalize its unit. observations:{' '}
              <span className="math">Q</span> distinct accepted observer envelopes, with production{' '}
              <span className="math">Q ≥ C</span>.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">deliberately not in the log</h3>
            <ul className="col-list">
              <li>wallet POST bodies, member BLS shares (stored against an accepted operation_id)</li>
              <li>FROST DKG / signing round packages</li>
              <li>BLS DKG commitments, reveals, secret shares</li>
              <li>peer status, AlephBFT frames, catch-up certificates — they <em>cover</em> the log</li>
            </ul>
          </div>
        </div>
      </Reveal>
    </>
  )
}