import { PageHead, Reveal, Callout, Steps, Step } from '../primitives.tsx'
import { Ledger } from '../figures/ledger.tsx'

const MELT_FLOW = [
  { label: 'MeltQuote', count: '1×', tone: 'g2' as const, note: 'first healthy member' },
  { label: 'Melt', count: '1×', tone: 'g1' as const, note: 'proofs reserved, change bound; no coins move' },
  { label: 'AddressAllocation', count: '0–1×', tone: 'g2' as const, note: 'internal change, if any' },
  { label: 'TransactionProposal', count: '1×', tone: 'secp' as const, note: 'exact unsigned tx: inputs, dest, fee, sighashes' },
  { label: 'TransactionSigned', count: '1×', tone: 'secp' as const, note: 'persisted before anyone talks to Bitcoin' },
  { label: 'BroadcastIntent', count: '1×', tone: 'secp' as const, note: 'those bytes only' },
  { label: 'BroadcastObservation', count: 'B×', tone: 'ok' as const, note: 'confirmations to quorum and depth' },
  { label: 'MeltQuotePayment', count: 'Q×', tone: 'ok' as const, note: 'then paid' },
]

export default function BdkMelt() {
  return (
    <>
      <PageHead
        idx="19"
        section="federated bdk · melt"
        title={
          <>
            Melt, step by step — <span className="cd">no redirect, no swap-in</span>
          </>
        }
        lead={
          <>
            Proofs in, Bitcoin out. The ledger trail from melt quote to broadcast is what stops one member from
            redirecting the payout or swapping in different bytes.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Ledger steps={MELT_FLOW} flow="Bitcoin moves, change is BLS" />
      </Reveal>
      <Reveal delay={0.2}>
        <Steps>
          <Step n={1}>
            wallet POSTs a melt quote with the destination address. <span className="math">make_payment</span> does
            not send coins — <span className="math">Melt</span> accept means proofs reserved/spent, change outputs
            bound, and an on-chain payment intent created.
          </Step>
          <Step n={2}>
            a member builds a candidate tx in watch-only BDK.{' '}
            <span className="math">TransactionProposal</span> carries the exact unsigned transaction — consensus does
            not take a PSBT on faith. every member recomputes ownership, value conservation, fee caps, destinations,
            and Taproot sighashes. conflict keys lock the batch, each UTXO, each intent.
          </Step>
          <Step n={3}>
            after accept, FROST per input: authorization binds operation + tx + input index + the exact 32-byte
            sighash. each signer rebuilds the sighash and refuses a wrong message. nonces go{' '}
            <span className="math">vacant → reserved → published → consumed</span>; failures burn, they do not reuse.
          </Step>
          <Step n={4}>
            <span className="math">TransactionSigned</span> persists the exact signed bytes, then{' '}
            <span className="math">BroadcastIntent</span> for those bytes only. any ready member may broadcast;
            observations cannot swap in another txid.
          </Step>
          <Step n={5}>
            confirmation observations to quorum and depth, then <span className="math">MeltQuotePayment</span> can
            mark paid. the wallet's melt status / change path fans out; change shares aggregate like mint shares.
          </Step>
        </Steps>
      </Reveal>
      <Reveal delay={0.28}>
        <Callout tone="rose" k="cannot">
          one member cannot redirect the payout, sign a different tx, broadcast unsigned or alternate bytes, or spend
          a UTXO that is not accepted-and-spendable. change, if any, is BLS and only for the accepted melt. plus
          off-log FROST rounds per input — no skipping from Melt to broadcast.
        </Callout>
      </Reveal>
    </>
  )
}