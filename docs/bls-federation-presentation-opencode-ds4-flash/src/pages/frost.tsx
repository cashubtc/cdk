import { PageHead, Reveal, Callout } from '../primitives.tsx'

export default function Frost() {
  return (
    <>
      <PageHead
        idx="17"
        section="the treasury key"
        title={
          <>
            FROST DKG — <span className="cd">threshold Schnorr</span>
          </>
        }
        lead={
          <>
            Startup runs two independent dealer-free ceremonies under the same roster, ceremony ID, and{' '}
            <span className="math">signature_threshold</span>: BLS12-381 for the eCash keysets, secp256k1 FROST for one
            application-neutral Bitcoin / Lightning root. Both must finish. A partial result never makes the mint ready.
          </>
        }
      />
      <Reveal delay={0.1}>
        <pre className="codeblock">{`setup
 ├─ BLS DKG   → k_i, K     Cashu proofs
 └─ FROST DKG → s_i, P     Bitcoin / Lightning   (same roster + same threshold, on purpose)

frost rounds (private plane, ciphersuite secp256k1_sha256_tr_v1)
 1. broadcast     /dkg/frost/round1   polynomial commitments + proofs
 2. confidential  /dkg/frost/round2   per-recipient contribution, wrong recipient rejected
 3. root confirm                      every member publishes the exact public bytes of P

then
 any t members → ordinary BIP340 Schnorr under P
 never reconstruct the private key`}</pre>
      </Reveal>
      <Reveal delay={0.16}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">round discipline</h3>
            <ul className="col-list">
              <li>every FROST message binds federation ID, ceremony ID, setup authorization, root epoch, protocol, ciphersuite, threshold, ordered roster, sender/receiver</li>
              <li>persist-before-send: sealed phase state written before any outbound package</li>
              <li>exact replay is idempotent; a second different payload is equivocation</li>
              <li>a phase does not advance until the batch is acknowledged</li>
              <li>at rest: signing share is AES-256-GCM ciphertext; public records never contain a plaintext share</li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-h">what comes out</h3>
            <p className="note" style={{ margin: 0 }}>
              public root package: root epoch, protocol/ciphersuite, threshold, ordered member → verification-share
              map, the compressed group key <span className="math">P</span>, transcript binding. local package, sealed:
              only that member's signing share <span className="math">s_i</span> — no mnemonic, no BDK descriptor, no
              Taproot tweak, no BLS material.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.22}>
        <ol className="recipe-list">
          <li>canonical untweaked root</li>
          <li>application-domain additive tweak</li>
          <li>domain-separated public chain code</li>
          <li>non-hardened BIP32 branch and child</li>
          <li>BIP340 even-Y normalization during FROST signing</li>
          <li>BIP341 key-path-only Taproot tweak for on-chain</li>
        </ol>
        <p className="note" style={{ marginTop: '0.5em' }}>
          Bark and BDK are consumers of this root. they do not run a second DKG for the treasury key — the DKG only
          creates the root, and everything else derives, in this exact order.
        </p>
      </Reveal>
      <Reveal delay={0.28}>
        <Callout k="one-liner">
          BLS DKG makes the eCash mint key; FROST DKG makes the treasury key; startup is not done until both exist and
          every member agrees on both public results. code: <span className="math">cdk-frost</span>,{' '}
          <span className="math">cdk-common/src/federation/frost_dkg.rs</span>, driver in{' '}
          <span className="math">cdk-axum/src/federation/dkg_driver.rs</span>.
        </Callout>
      </Reveal>
    </>
  )
}