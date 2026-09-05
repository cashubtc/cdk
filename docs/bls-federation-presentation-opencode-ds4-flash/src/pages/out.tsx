import { PageHead, Reveal, Callout, Quote, Steps, Step } from '../primitives.tsx'

export default function Out() {
  return (
    <>
      <PageHead
        idx="24"
        section="open questions"
        title={
          <>
            What is <span className="cd">still open</span>
          </>
        }
        lead={
          <>
            The notes end where the research ends. These are the questions to confirm before this becomes a finished
            talk — and the one-liners worth keeping on the final slide.
          </>
        }
      />
      <Reveal delay={0.1}>
        <Steps>
          <Step n={1}>
            is there a published NUT text for keyset v3 / BLS DHKE to quote, or are we presenting CDK's implementation
            ahead of the spec writeup?
          </Step>
          <Step n={2}>
            which exact NUT-00 / nutshell test vectors belong on slides — and do we want a live demo of a standalone v3
            mint, or equations plus code pointers?
          </Step>
          <Step n={3}>
            for the federation half: should FROST / Bark be a later appendix so the first talk stays on Cashu issuance?
          </Step>
          <Step n={4}>
            “then why not FROST-sign the secret instead of BLS-DHKE?” — a third design that replaces Chaumian blinding
            with threshold Schnorr. that is not Cashu anymore; a different eCash scheme. footnote, not a fork.
          </Step>
        </Steps>
      </Reveal>
      <Reveal delay={0.18}>
        <Quote>
          Bare secp multisig federates the token. BLS threshold federates the issuer — and then emits a normal token.
          Cashu cares about the second thing. The bearer object should stay boring.
        </Quote>
      </Reveal>
      <Reveal delay={0.26}>
        <Callout k="recap">
          single-signer BLS first, then a threshold of it: keyset v3 switches the algebra · pairings replace DLEQ ·
          Shamir shares stay on the issuance side · consensus-before-sign kills mix-and-match · two planes, one
          federation · DKG so no one ever holds <span className="math">k</span> · SIG_ALL is the remaining client-intent
          hole · the roster is the constitution · peers give you the book, backup gives you the pen.
        </Callout>
      </Reveal>
      <Reveal delay={0.3}>
        <p className="note" style={{ marginTop: '1em', textAlign: 'center' }}>
          content: <span className="mono">docs/FEDERATION_NOTES.md</span> · branches:{' '}
          <span className="mono">main → bls → bls-federation</span> · key files:{' '}
          <span className="mono">crates/cashu/src/nuts/nut01/bls.rs</span>,{' '}
          <span className="mono">crates/cdk/src/wallet/federation.rs</span>,{' '}
          <span className="mono">cdk-common/src/federation/</span>, <span className="mono">cdk-frost</span>
        </p>
      </Reveal>
    </>
  )
}