import { PageHead, Reveal, Callout, Steps, Step } from '../primitives.tsx'
import { Ledger } from '../figures/ledger.tsx'

const MINT_FLOW = [
  { label: 'MintQuote', count: '1×', tone: 'g2' as const, note: 'first healthy member' },
  { label: 'AddressAllocation', count: '1×', tone: 'g2' as const, note: 'binds quote · epoch · keychain · index' },
  { label: 'ChainObservation::Deposit', count: 'D×', tone: 'secp' as const, note: 'watch-only, mempool is not payment' },
  { label: 'MintQuotePayment', count: 'Q×', tone: 'ok' as const, note: 'observers must agree, then paid' },
  { label: 'Mint', count: '1×', tone: 'g1' as const, note: 'blinded outputs, conflict = quote' },
]

export default function BdkMint() {
  return (
    <>
      <PageHead
        idx="18"
        section="federated bdk · mint"
        title={
          <>
            Mint, step by step — <span className="cd">quorum before eCash</span>
          </>
        }
        lead={
          <>
            The two public Cashu calls look normal. Everything that makes federated BDK safe is hidden on the private
            plane: address allocation, chain observations, payment quorums — then BLS shares.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Ledger steps={MINT_FLOW} flow="BLS shares → v3 proof" />
      </Reveal>
      <Reveal delay={0.2}>
        <Steps>
          <Step n={1}>
            wallet POSTs a mint quote — connector tries the <em>first healthy member</em>, not full fan-out.
          </Step>
          <Step n={2}>
            that member submits two consensus operations, <span className="math">MintQuote</span> and{' '}
            <span className="math">AddressAllocation</span>. after accept, every honest member derives the same
            address from the FROST/BIP32 descriptor. replay can only reveal that exact index.
          </Step>
          <Step n={3}>
            user pays; members' watch-only BDK wallets see a UTXO — <em>a cache, not truth</em>. each watcher submits a
            deposit observation: outpoint, amount, script, block, depth, allocation.
          </Step>
          <Step n={4}>
            <span className="math">MintQuotePayment</span> finalizes <span className="math">amount_paid</span> only
            after the observation quorum <span className="math">Q</span> agrees on quote, method, state, amounts,
            payment identity. one “I saw it” does not issue eCash.
          </Step>
          <Step n={5}>
            wallet fans <span className="math">/mint</span> with blinded outputs; each member wraps the exact outputs
            as a <span className="math">Mint</span> envelope, waits for accept, returns a BLS share. wallet verifies
            shares against <span className="math">K_i</span> and interpolates one v3 proof.
          </Step>
        </Steps>
      </Reveal>
      <Reveal delay={0.28}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">checks that matter</h3>
            <ul className="col-list">
              <li>address is consensus-owned</li>
              <li>mempool is not payment</li>
              <li>reorg before issuance unsays the UTXO</li>
              <li>reorg after issuance alarms, does not unmint</li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">typical 3-of-5, Q = C = 4</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">1 + 1 + 4 + 4 + 1 = 11</span> distinct accepted operations for one mint. a reorg
              adds Reorg observations and a new deposit epoch.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.34}>
        <Callout k="observations are one-per-observer">
          quorum is counted across <em>accepted</em> envelopes, not by stuffing many observers into one message.
          production <span className="math">member_quorum ≥ consensus_threshold</span>;
          <span className="math">development_single_observation</span> is test-only trust in one local observer.
        </Callout>
      </Reveal>
    </>
  )
}