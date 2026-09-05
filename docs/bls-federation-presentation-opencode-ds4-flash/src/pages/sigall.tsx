import { PageHead, Reveal, Callout, Step, Tag } from '../primitives.tsx'

export default function SigAll() {
  return (
    <>
      <PageHead
        idx="15"
        section="still open"
        title={
          <>
            SIG_ALL and <span className="cr">client spend intent</span>
          </>
        }
        lead={
          <>
            Consensus stops mix-and-match among honest members. It does not stop one Byzantine member from spending the
            wallet's proofs first. Audit name: <span className="math">SEC-2026-07-17-01</span> — still a production
            blocker in the implementation plan.
          </>
        }
      />
      <Reveal delay={0.1}>
        <pre className="codeblock">{`wallet fans   P1,P2 → A,B     to M1..M5

evil M5 copies P1,P2
     builds  P1,P2 → X,Y
     submits that envelope first

consensus box
   winner: P1,P2 → X,Y        ← balanced, well-formed, accepted

honest members
   spend P1,P2
   sign X,Y                   ← attacker outputs

wallet
   P1,P2 → A,B is now spent-against`}</pre>
      </Reveal>
      <Reveal delay={0.18}>
        <Step n={1}>
          A normal Cashu proof is a bearer instrument — whoever can present <span className="math">(secret, C)</span>{' '}
          can spend it. the operation is authorized as “a federation member said this swap should happen,” not “the
          owner of these proofs wanted these outputs.”
        </Step>
      </Reveal>
      <Reveal delay={0.22}>
        <Step n={2}>
          the database still prevents two committed spends. the attacker spend wins; the user spend loses. this is not
          the sliding-window attack — that needs different members signing different windows; this needs only one evil
          member that saw the proofs.
        </Step>
      </Reveal>
      <Reveal delay={0.26}>
        <Step n={3}>
          <Tag c="ck">NUT-11 already has the tool</Tag>: lock the proof to P2PK and sign with{' '}
          <span className="math">SIG_ALL</span>. that witness commits the owner to the exact outputs of the spend —
          changing <span className="math">A,B</span> to <span className="math">X,Y</span> invalidates the signature,
          and admission can reject the rewrite before it becomes the accepted operation.
        </Step>
      </Reveal>
      <Reveal delay={0.3}>
        <Callout tone="rose" k="status">
          the current code can bind SIG_ALL when the user already used it, and tests prove a SIG_ALL output mutation
          changes the operation ID and fails apply. it does <em>not</em> require SIG_ALL on every federated spend.
          until that protocol decision is made, the federation should not be described as safe for users against one
          malicious member.
        </Callout>
      </Reveal>
    </>
  )
}