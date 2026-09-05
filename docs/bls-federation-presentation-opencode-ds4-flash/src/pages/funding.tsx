import { PageHead, Reveal, Callout, Steps, Step, Tag, Quote } from '../primitives.tsx'

export default function Funding() {
  return (
    <>
      <PageHead
        idx="16"
        section="the other half"
        title={
          <>
            The mint is <span className="cd">two machines</span>
          </>
        }
        lead={
          <>
            One machine signs proofs — that is the BLS federation. The other holds the real money: Lightning or
            on-chain. If you only federate the first, you have a committee that prints IOUs against a piggy bank one
            person still holds.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Steps>
          <Step n={1}>
            <Tag c="cr">why classical connectors are not enough</Tag> — point every federation member at one CLN/LND/BDK
            node and that node's operator can mark a quote paid when nothing arrived, refuse to pay a melt, or sweep
            the funds while the eCash ledger still looks honest. the other members can vote all they want; they cannot
            stop a key they do not share.
          </Step>
          <Step n={2}>
            the funding backend must give every member the <em>same grade of access</em>: same view of deposits and
            invoices, same ability to refuse a spend, same inability to spend alone. the quorum that issues eCash must
            be the quorum that moves the reserves — if they diverge, the smaller one is the real mint.
          </Step>
          <Step n={3}>
            two federated backends: <Tag c="c2">federated BDK</Tag> — the on-chain treasury. members watch the chain,
            agree on addresses and unsigned transactions through the same consensus box, then jointly sign. one member
            can propose “send these coins to this melt address”; it cannot actually move them.
          </Step>
          <Step n={4}>
            <Tag c="c2">federated Bark</Tag> — the Lightning treasury. invoices and payments are planned as consensus
            operations first; claiming an incoming HTLC or sending an outgoing payment is a threshold custody action,
            not “member 3's CLN paid it.”
          </Step>
          <Step n={5}>
            both use <Tag c="secp">FROST on secp256k1</Tag> — a different key, on a different curve, for a different
            job. BLS blinds and signs Cashu; FROST signs Bitcoin/Lightning the outside world understands. do not mix
            the secrets. setup is only complete when both ceremonies exist.
          </Step>
        </Steps>
      </Reveal>
      <Reveal delay={0.2}>
        <Quote>
          If three members can issue proofs but one member can invent a “paid” observation, eCash inflates. If three
          members must agree the quote is paid, but one member can later sweep the deposit, the remaining proofs are
          unbacked.
        </Quote>
      </Reveal>
      <Reveal delay={0.26}>
        <Callout k="the blunt rule">
          same roster, same consensus history, and a signing threshold that cannot be weaker than the eCash threshold
          in a way that lets a minority move funds or invent deposits. fakewallet and{' '}
          <span className="math">development_single_observation</span> exist for tests — they explicitly trust one
          local observation. that is not a federation funding model.
        </Callout>
      </Reveal>
    </>
  )
}