import { PageHead, Reveal, Callout, Tag } from '../primitives.tsx'
import { Sliding } from '../figures/sliding.tsx'

export default function MixMatch() {
  return (
    <>
      <PageHead
        idx="11"
        section="the attack"
        title={
          <>
            Threshold-signing <span className="cr">mix-and-match</span>
          </>
        }
        lead={
          <>
            The motivating attack of the whole federation design. Informal talk calls it the sliding-window attack;
            the RFC names it threshold-signing mix-and-match — a wallet presents different outputs to different
            members and combines overlapping share subsets.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Sliding />
      </Reveal>
      <Reveal delay={0.22}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">why it exists</h3>
            <p className="note" style={{ margin: 0 }}>
              A share is independently valid: <span className="math">C_i = k_i·B</span> checks against member{' '}
              <span className="math">i</span>'s public share even if nobody else signed that <span className="math">B</span>.
              If members sign what they are shown, a wallet can pay once, rotate windows, and interpolate more
              finished proofs than the quote authorized.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">not a BLS bug</h3>
            <p className="note" style={{ margin: 0 }}>
              This is what happens when a threshold issuer signs <em>before</em> it agrees on the exact message. Same
              quote or same inputs plus a different output set is a conflict — not a second signing job. That is why
              the official name is used: <Tag c="cr">mix-and-match</Tag>, with <Tag>sliding window</Tag> reserved for
              the overlapping-subset picture.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.28}>
        <pre className="codeblock">{`# the 4-window regression picture (Milestone 12)
Outputs: A B C D   (4 notes)     Paid for: 3

Window ABC -> some members        M1 signs A B C
Window BCD -> the next members    M2 signs B C D
Window CDA -> the next            M3 signs C D A
Window DAB -> the next            M4 signs D A B
                                  M5 signs A B C

A: M1,M3,M4,M5   B: M1,M2,M4,M5   C: M1,M2,M3,M5   D: M2,M3,M4
# four finished proofs from a three-token payment`}</pre>
      </Reveal>
      <Reveal delay={0.34}>
        <Callout tone="rose" k="consensus is the fix">
          the tests ship a naive “sign whatever you were shown” double precisely so the suite can beat it:{' '}
          <span className="math">runtime_config_rejects_sliding_window_mint_output_attack</span> and{' '}
          <span className="math">local_federated_members_reject_sliding_window_swap_output_attack</span>.
        </Callout>
      </Reveal>
    </>
  )
}