import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react'

type Accent = 'cyan' | 'violet' | 'amber' | 'coral'

type SlideKind =
  | 'branch'
  | 'naming'
  | 'commits'
  | 'why-bls'
  | 'keyset'
  | 'crypto'
  | 'derivation'
  | 'lifecycle'
  | 'safety'
  | 'files'
  | 'chapters'
  | 'teaser'
  | 'questions'
  | 'alternative'
  | 'fork'
  | 'threshold'
  | 'attack'
  | 'consensus'
  | 'transport'
  | 'dkg'
  | 'sigall'
  | 'funding'
  | 'frost'
  | 'bdk'
  | 'swap'
  | 'wire'
  | 'roster'
  | 'recovery'

type SlideSpec = {
  id: string
  kind: SlideKind
  kicker: string
  title: string
  deck: string
  accent: Accent
  bullets?: string[]
  takeaway?: string
}

const slides: SlideSpec[] = [
  {
    id: 'branch-map',
    kind: 'branch',
    kicker: 'orientation / branch map',
    title: 'Two stories. One protocol surface.',
    deck: 'Start with the original BLS v3 transition, then layer federation on top. The implementation only makes sense in that order.',
    accent: 'cyan',
    bullets: [
      'main → standalone CDK baseline',
      'bls → BLS12-381, keyset v3, single signer',
      'bls-federation → threshold issuance, consensus, custody',
    ],
    takeaway: 'Do not mix the precursor and the federation story in the same chapter.',
  },
  {
    id: 'naming-trap',
    kind: 'naming',
    kicker: 'orientation / naming trap',
    title: 'v3 is Version02 in the code.',
    deck: 'The user-facing version, Rust enum, and ID prefix are deliberately offset. The token encoding version is a different axis entirely.',
    accent: 'violet',
    bullets: [
      'v1 / Version00 / 00 + 7 bytes / secp256k1',
      'v2 / Version01 / 01 + 32 bytes / secp256k1',
      'v3 / Version02 / 02 + 32 bytes / BLS12-381',
    ],
    takeaway: '“TokenV3” is not “BLS keyset v3”.',
  },
  {
    id: 'original-commits',
    kind: 'commits',
    kicker: 'precursor / original bls commits',
    title: 'The hardening trail is part of the design.',
    deck: 'The original branch was small, but the follow-up fixes define the sharp edges: canonical scalars, blind verification, and mixed-curve safety.',
    accent: 'amber',
    bullets: [
      'BLS types + versioned DHKE + v3 keysets',
      'rejection sampling replaces unsafe modular reduction',
      'blind pairing verification closes the mint-response gap',
    ],
    takeaway: 'A cryptographic feature is not landed when the happy path compiles.',
  },
  {
    id: 'why-bls',
    kind: 'why-bls',
    kicker: 'precursor / why bls',
    title: 'Threshold-ready without changing the bearer object.',
    deck: 'secp DHKE is additive. BLS moves verification to pairings, so a secret can be split while a wallet still checks one public mint key.',
    accent: 'violet',
    bullets: [
      'secp: Y = H(s), B = Y + rG, C = C_ − rK',
      'BLS: B = r · H(s), C = r⁻¹ · C_, e(C, G2) = e(H(s), K)',
      'the original bls branch is still single-signer',
    ],
    takeaway: 'The pairing is the bridge from one operator to a shared secret.',
  },
  {
    id: 'keyset-v3',
    kind: 'keyset',
    kicker: 'precursor / keyset v3 transition',
    title: 'A protocol switch, not a new token format.',
    deck: 'v3 reuses the v2 keyset-ID commitment recipe, changes the curve, and keeps live secp keysets intact until an operator rotates them.',
    accent: 'cyan',
    bullets: [
      'ID commits to amount keys, unit, fees, and expiry',
      'same 33-byte ID width; first byte becomes 02',
      'new keysets default to v3 when no legacy flag is set',
    ],
    takeaway: 'Compatibility lives in the keyset version boundary.',
  },
  {
    id: 'crypto-building-blocks',
    kind: 'crypto',
    kicker: 'precursor / crypto building blocks',
    title: 'Three types carry the curve boundary.',
    deck: 'PublicKey becomes a tagged union, not a universal secp point. Length-based parsing is useful; context-aware verification is mandatory.',
    accent: 'violet',
    bullets: [
      'G1: 48-byte compressed blinded messages and signatures',
      'G2: 96-byte compressed mint public keys',
      'Fr*: canonical, non-zero scalars for keys and weights',
    ],
    takeaway: 'BLS changes the type system before it changes the API.',
  },
  {
    id: 'deterministic-derivation',
    kind: 'derivation',
    kicker: 'precursor / deterministic derivation',
    title: 'Restore must follow the keyset curve.',
    deck: 'NUT-13 wallet secrets and NUT-02 amount keys both branch on v3. Rejection sampling makes deterministic BLS scalars canonical instead of merely reduced.',
    accent: 'amber',
    bullets: [
      'wallet: HMAC-SHA256 KDF keyed by seed + keyset ID',
      'mint: hardened amount path plus an attempt index',
      'zero and non-canonical Fr values are rejected',
    ],
    takeaway: 'The derivation path is part of the protocol, not an implementation detail.',
  },
  {
    id: 'proof-life-cycle',
    kind: 'lifecycle',
    kicker: 'precursor / proof life cycle',
    title: 'Verify twice. Unblind once.',
    deck: 'A v3 response is trusted only after the blind signature pairing passes. The final proof gets its own pairing after unblinding.',
    accent: 'cyan',
    bullets: [
      'blind: B = r · H(secret)',
      'mint: C_ = k · B, with no DLEQ',
      'wallet: e(C_, G2) = e(B, K), then C = r⁻¹ · C_',
    ],
    takeaway: 'Pairings replace DLEQ for the v3 verification path.',
  },
  {
    id: 'mixed-curve-safety',
    kind: 'safety',
    kicker: 'precursor / mixed-curve safety',
    title: 'Valid bytes can still be invalid context.',
    deck: 'A compressed G1 point can parse as a PublicKey and still be forbidden in a secp-only P2PK or HTLC condition. The fix is explicit errors, not optimism.',
    accent: 'coral',
    bullets: [
      'identity / infinity points rejected at parse time',
      'P2PK and HTLC stay secp-only',
      'fuzzers inject valid BLS points to prove “error, not panic”',
    ],
    takeaway: 'Mixed curves turn old helper assumptions into attack surface.',
  },
  {
    id: 'bls-files',
    kind: 'files',
    kicker: 'precursor / implementation map',
    title: 'Follow the curve through the stack.',
    deck: 'The BLS/v3 story crosses protocol types, DHKE, keysets, wallet sagas, signatory validation, and fuzz targets. No single crate owns the transition.',
    accent: 'cyan',
    bullets: [
      'cashu: nut01, nut02, dhke, nut12, nut13',
      'cdk: mint builder, keyset rotation, wallet sagas',
      'signatory + mintd + fuzz: enforcement and safety net',
    ],
    takeaway: 'Protocol versioning is a vertical slice.',
  },
  {
    id: 'suggested-chapters',
    kind: 'chapters',
    kicker: 'precursor / suggested chapters',
    title: 'Four chapters make the BLS half legible.',
    deck: 'Before discussing federation, establish the algebra, the keyset boundary, the proof walk, and the compatibility minefield.',
    accent: 'violet',
    bullets: [
      '01 why BLS exists before federation',
      '02 keyset v3 is the protocol switch',
      '03 how a v3 proof is born and checked',
      '04 mixed-curve types are a safety problem',
    ],
    takeaway: 'The federation story starts only after the standalone v3 story is boring.',
  },
  {
    id: 'federation-teaser',
    kind: 'teaser',
    kicker: 'federation / teaser',
    title: 'The next layer changes who may sign.',
    deck: 'Federation adds threshold shares, operation ordering, member transport, and a second threshold ceremony. The wallet still wants ordinary Cashu.',
    accent: 'amber',
    bullets: [
      'threshold primitives and v3-only federation gates',
      'DKG + keyset rotation + wallet aggregation',
      'consensus, payments, and transport as operational layers',
    ],
    takeaway: 'The goal is a shared issuer, not a federation-shaped token.',
  },
  {
    id: 'open-questions',
    kind: 'questions',
    kicker: 'federation / open questions',
    title: 'The research notes leave useful seams exposed.',
    deck: 'Some choices are implementation questions. Others are talk-shaping decisions that change what we claim the system is.',
    accent: 'coral',
    bullets: [
      'spec text and test vectors: what is normative?',
      'live standalone v3 mint demo or equations only?',
      'FROST/Bark: core story or a later appendix?',
    ],
    takeaway: 'Do not turn an open product decision into a protocol guarantee.',
  },
  {
    id: 'secp-alternative',
    kind: 'alternative',
    kicker: 'design fork / alternative',
    title: 'Bare multisig would federate the token.',
    deck: 'Each member could return a complete secp signature and the wallet could carry t of them. That is a federation, but every later Cashu hop inherits the roster.',
    accent: 'coral',
    bullets: [
      'proof becomes a bundle of signatures plus policy',
      'receive, swap, melt, restore, and APIs become federation-aware',
      'threshold is moved into the bearer object',
    ],
    takeaway: 'Cashu wants the bearer object to stay boring.',
  },
  {
    id: 'design-fork',
    kind: 'fork',
    kicker: 'design fork / the decision',
    title: 'BLS threshold federates the issuer.',
    deck: 'Interpolated secp can produce one point, but it needs a new aggregate proof story. Pairings verify each share and the final proof with the same equation family.',
    accent: 'violet',
    bullets: [
      'bare secp multisig: t checks, t signatures, roster-shaped money',
      'BLS shares: one C, one K, one ordinary v3 proof',
      'both still need consensus; crypto does not order operations',
    ],
    takeaway: 'Naive multisig solves availability. BLS solves composability.',
  },
  {
    id: 'threshold-scheme',
    kind: 'threshold',
    kicker: 'federation / threshold BLS',
    title: 'One polynomial. Many shares. One final C.',
    deck: 'For each denomination, Shamir shares one BLS scalar k. Any t member shares interpolate the same blinded signature in the exponent.',
    accent: 'violet',
    bullets: [
      'f(x) = a₀ + a₁x + … + aₜ₋₁xᵗ⁻¹, k = f(0)',
      'member i: kᵢ = f(i), Cᵢ = kᵢ · B',
      'C = Σ λᵢ(0) · Cᵢ = k · B',
    ],
    takeaway: 'The wallet reconstructs a signature, never the secret.',
  },
  {
    id: 'mix-and-match',
    kind: 'attack',
    kicker: 'federation / threshold-signing mix-and-match',
    title: 'Valid shares can still be for different messages.',
    deck: 'If members sign whatever blinded outputs they see, overlapping windows can mint more than a quote or input set authorized. This is an ordering problem, not a BLS bug.',
    accent: 'coral',
    bullets: [
      '3-of-5, paid for 3 outputs, four windows cover A–D',
      'each member sees a locally balanced request',
      'the wallet combines enough valid shares for an unpaid output',
    ],
    takeaway: 'Threshold signing must happen after the exact operation is accepted.',
  },
  {
    id: 'consensus-before-signing',
    kind: 'consensus',
    kicker: 'federation / consensus before signing',
    title: 'Consensus names the transaction first.',
    deck: 'AlephBFT is the black box that turns an envelope into one accepted history. Members sign because the exact operation won ordering, not because an HTTP request arrived.',
    accent: 'amber',
    bullets: [
      'operation_id = hash(federation + kind + inputs + outputs)',
      'same inputs, different outputs → different conflicting envelope',
      'apply spends inputs once, then emits only accepted shares',
    ],
    takeaway: 'BLS is the compact signature after consensus, not a substitute for it.',
  },
  {
    id: 'data-flow-transport',
    kind: 'transport',
    kicker: 'federation / data flow + transport',
    title: 'Two planes. Two topologies. One mint.',
    deck: 'The wallet is a fan-out hub on the public plane. Members form a private mesh for consensus and DKG. HTTP and Iroh are interchangeable pipes.',
    accent: 'cyan',
    bullets: [
      'public: ordinary Cashu request → member BLS shares',
      'private: authenticated /federation/v1 → consensus + DKG',
      'transport is not the trust boundary; topology is',
    ],
    takeaway: 'The wallet aggregates. The members agree.',
  },
  {
    id: 'bls-dkg',
    kind: 'dkg',
    kicker: 'federation / BLS DKG',
    title: 'The full mint key never exists in one room.',
    deck: 'Dealer-free Pedersen DKG makes each member contribute a polynomial. Public commitments and private evaluations add into the same Shamir share model used for issuance.',
    accent: 'violet',
    bullets: [
      'commit first, reveal later, so coefficients cannot adapt',
      'private evaluations are checked against G2 commitments',
      'sum verified shares → kᵢ; sum constant commitments → K',
    ],
    takeaway: 'Hash → reveal → deliver → add → activate.',
  },
  {
    id: 'sig-all',
    kind: 'sigall',
    kicker: 'security / missing SIG_ALL',
    title: 'Consensus cannot authenticate client intent.',
    deck: 'A member that sees bearer proofs can rewrite a swap and race it into consensus. The current notes call SIG_ALL binding a production blocker, not a solved detail.',
    accent: 'coral',
    bullets: [
      'consensus proves a member authored the envelope',
      'it does not prove the proof owner chose those outputs',
      'P2PK + SIG_ALL binds the user to the exact spend',
    ],
    takeaway: 'A valid BLS share for the wrong outputs is still theft.',
  },
  {
    id: 'funding-backends',
    kind: 'funding',
    kicker: 'treasury / funding backends',
    title: 'An eCash federation needs a federated treasury.',
    deck: 'BLS protects issuance. BDK and Bark protect the reserves. A single CLN, LND, or wallet behind the committee leaves one operator as the real mint.',
    accent: 'amber',
    bullets: [
      'Federated BDK: consensus-shaped on-chain intents + FROST',
      'Federated Bark: threshold-held Lightning receive/send actions',
      'same economic quorum, separate key hierarchy',
    ],
    takeaway: 'The smaller quorum is the real authority.',
  },
  {
    id: 'frost-dkg',
    kind: 'frost',
    kicker: 'treasury / FROST DKG',
    title: 'Two ceremonies. Two jobs. Do not collapse them.',
    deck: 'BLS DKG creates the Cashu mint keyset on G2. FROST DKG creates the Bitcoin/Lightning custody key on secp256k1. Same roster, different threat model.',
    accent: 'amber',
    bullets: [
      'BLS: blinded eCash signatures',
      'FROST: external Schnorr signatures the chain understands',
      'setup is incomplete until both roots exist and are bound',
    ],
    takeaway: 'Threshold is a technique; the key’s job still matters.',
  },
  {
    id: 'bdk-mint-melt',
    kind: 'bdk',
    kicker: 'treasury / BDK mint + melt',
    title: 'Money in, proofs out. Proofs in, money out.',
    deck: 'Federated BDK turns address allocation, observation, transaction proposals, signing, and broadcast into ordered application facts.',
    accent: 'amber',
    bullets: [
      'mint: observe deposit → quorum payment → issue proofs',
      'melt: spend proofs → approve intent → sign/broadcast payout',
      'one member can propose; no member can move funds alone',
    ],
    takeaway: 'Mint and melt are ledger actions plus treasury actions.',
  },
  {
    id: 'swap-step-by-step',
    kind: 'swap',
    kicker: 'application / swap',
    title: 'Swap is a named operation, not a bag of messages.',
    deck: 'The request keeps its ordinary Cashu shape, but federation wraps it in an envelope, orders it, applies it idempotently, and only then releases shares.',
    accent: 'cyan',
    bullets: [
      'wallet fans the same inputs + outputs to every member',
      'members submit the same operation_id to consensus',
      'accepted inputs are spent once; output shares aggregate normally',
    ],
    takeaway: 'Replay the same envelope; reject a mutated one.',
  },
  {
    id: 'consensus-wire',
    kind: 'wire',
    kicker: 'application / consensus messages',
    title: 'The log stores named facts, not every packet.',
    deck: 'AlephBFT orders application envelopes. Signature shares, HTTP bodies, FROST rounds, and transport frames live beside the log, bound to the operation or session they serve.',
    accent: 'cyan',
    bullets: [
      'swap: 1 ordered operation + t off-log shares',
      'federated mint: quote, address, observations, payment, issue',
      'catch-up replays facts; it does not invent new kinds',
    ],
    takeaway: 'Consensus messages are the history; transport is the delivery mechanism.',
  },
  {
    id: 'roster-config',
    kind: 'roster',
    kicker: 'constitution / roster + config',
    title: 'The roster is a constitution.',
    deck: 'FederationId freezes membership, thresholds, URLs, and identity keys. ConfigDigest binds the rest. Changing DNS can be as fundamental as changing a key.',
    accent: 'violet',
    bullets: [
      'federation_id = domain-separated hash of setup + roster',
      'config_digest = hash of the full public configuration',
      'keyset rotation is live; roster mutation is recreate-and-migrate',
    ],
    takeaway: 'If a hostname is in the constitution, changing it founds a new country.',
  },
  {
    id: 'crash-recovery',
    kind: 'recovery',
    kicker: 'operations / crash resistance',
    title: 'Peers can give you the book. Only backup gives you the pen.',
    deck: 'Journal, checkpoints, and catch-up can rebuild state. Identity keys, BLS shares, and the sealed FROST root are member-local and cannot be reconstructed from peers.',
    accent: 'coral',
    bullets: [
      'DB loss with secrets: restore or replay from genesis',
      'network lag: catch up and fail closed until digests match',
      'secret loss: the seat is dead; do not copy another member',
    ],
    takeaway: 'A half-restored member never signs.',
  },
]

const versionRows = [
  { label: 'v1', enum: 'Version00', id: '00 + 7 bytes', curve: 'secp256k1', note: 'legacy' },
  { label: 'v2', enum: 'Version01', id: '01 + 32 bytes', curve: 'secp256k1', note: 'fees / expiry' },
  { label: 'v3', enum: 'Version02', id: '02 + 32 bytes', curve: 'BLS12-381', note: 'threshold-ready' },
]

const lifecycleSteps = [
  { label: 'blind', short: 'B', detail: 'B = r · H(secret)', tone: 'violet' },
  { label: 'sign', short: 'C_', detail: 'C_ = k · B', tone: 'cyan' },
  { label: 'check blind', short: 'e₁', detail: 'e(C_, G2) = e(B, K)', tone: 'amber' },
  { label: 'unblind', short: 'C', detail: 'C = r⁻¹ · C_', tone: 'violet' },
  { label: 'check proof', short: 'e₂', detail: 'e(C, G2) = e(H(secret), K)', tone: 'cyan' },
]

const dkgSteps = [
  { label: 'commit', detail: 'H(revealⱼ)', code: 'sealed transcript' },
  { label: 'reveal', detail: '[fⱼ(0)G2 …]', code: 'public commitments' },
  { label: 'deliver', detail: 'fⱼ(i)', code: 'private evaluation' },
  { label: 'sum', detail: 'kᵢ = Σ fⱼ(i)', code: 'verified share' },
  { label: 'activate', detail: 'K = Σ fⱼ(0)G2', code: 'v3 keyset' },
]

const operationKinds = [
  { label: 'Swap', count: '1', tone: 'cyan' },
  { label: 'Mint', count: '1 + 1 + Q + 1', tone: 'violet' },
  { label: 'Melt', count: '1 + 1 + 1 + 1 + B + Q', tone: 'amber' },
  { label: 'KeysetRotation', count: '1', tone: 'coral' },
]

function App() {
  const [activeIndex, setActiveIndex] = useState(0)
  const [visible, setVisible] = useState<Set<number>>(() => new Set([0]))
  const slideRefs = useRef<Array<HTMLElement | null>>([])
  const allSlides = useMemo(() => ['cover', ...slides.map((slide) => slide.id)], [])

  useEffect(() => {
    const observer = new IntersectionObserver(
      (entries) => {
        const visibleEntries = entries.filter((entry) => entry.isIntersecting)
        if (!visibleEntries.length) return

        setVisible((previous) => {
          const next = new Set(previous)
          visibleEntries.forEach((entry) => {
            const index = Number((entry.target as HTMLElement).dataset.slideIndex)
            next.add(index)
          })
          return next
        })

        const strongest = visibleEntries.sort((a, b) => b.intersectionRatio - a.intersectionRatio)[0]
        setActiveIndex(Number((strongest.target as HTMLElement).dataset.slideIndex))
      },
      { threshold: [0.44, 0.62, 0.82], rootMargin: '-4% 0px -4% 0px' },
    )

    slideRefs.current.forEach((element) => {
      if (element) observer.observe(element)
    })

    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return
      if (event.key === 'ArrowDown' || event.key === 'PageDown' || event.key === ' ') {
        event.preventDefault()
        goTo(activeIndex + 1, false)
      }
      if (event.key === 'ArrowUp' || event.key === 'PageUp') {
        event.preventDefault()
        goTo(activeIndex - 1, false)
      }
      if (event.key === 'Home') {
        event.preventDefault()
        goTo(0, false)
      }
      if (event.key === 'End') {
        event.preventDefault()
        goTo(allSlides.length - 1, false)
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [activeIndex, allSlides.length])

  const goTo = (index: number, smooth = true) => {
    const safeIndex = Math.max(0, Math.min(index, allSlides.length - 1))
    const node = slideRefs.current[safeIndex]
    if (!node) return
    node.scrollIntoView({ behavior: smooth ? 'smooth' : 'auto', block: 'start' })
  }

  return (
    <div className="app-shell">
      <header className="topbar">
        <a className="wordmark" href="#cover" onClick={() => goTo(0)}>
          <span className="wordmark__mark">◌</span>
          <span>CDK / BLS-FEDERATION</span>
        </a>
        <div className="topbar__status">
          <span className="live-dot" />
          <span>field notes · 2026</span>
        </div>
        <div className="topbar__hint">scroll to traverse / ↑ ↓ to jump</div>
      </header>

      <nav className="progress-rail" aria-label="Presentation sections">
        <div className="progress-rail__line" aria-hidden="true">
          <span style={{ height: `${((activeIndex + 1) / allSlides.length) * 100}%` }} />
        </div>
        <div className="progress-rail__dots">
          {allSlides.map((id, index) => (
            <button
              key={id}
              className={`progress-dot ${activeIndex === index ? 'is-active' : ''}`}
              type="button"
              aria-label={`Go to ${index === 0 ? 'cover' : slides[index - 1].title}`}
              aria-current={activeIndex === index ? 'step' : undefined}
              onClick={() => goTo(index)}
            >
              <span>{String(index).padStart(2, '0')}</span>
            </button>
          ))}
        </div>
      </nav>

      <main className="deck">
        <section
          ref={(node) => {
            slideRefs.current[0] = node
          }}
          data-slide-index="0"
          id="cover"
          className={`cover slide ${visible.has(0) ? 'is-visible' : ''}`}
          aria-labelledby="cover-title"
        >
          <div className="cover__grid">
            <div className="cover__copy reveal-group">
              <div className="eyebrow"><span className="eyebrow__index">00</span> protocol trace / presentation</div>
              <p className="cover__overline">THE ISSUER SPLITS.</p>
              <h1 id="cover-title">The proof<br /><em>doesn’t.</em></h1>
              <p className="cover__dek">BLS, threshold issuance, and the federation layers that keep Cashu’s bearer object ordinary.</p>
              <div className="cover__meta">
                <span>derived from</span>
                <code>docs/FEDERATION_NOTES.md</code>
              </div>
              <button className="jump-button" type="button" onClick={() => goTo(1)}>
                <span>Enter the trace</span>
                <span className="jump-button__arrow">↓</span>
              </button>
            </div>
            <CoverFigure />
          </div>
          <div className="cover__footer"><span>cashu development kit</span><span>01 / 29</span></div>
        </section>

        {slides.map((slide, index) => {
          const pageIndex = index + 1
          return (
            <section
              key={slide.id}
              ref={(node) => {
                slideRefs.current[pageIndex] = node
              }}
              data-slide-index={pageIndex}
              id={slide.id}
              className={`slide chapter slide--${slide.kind} ${visible.has(pageIndex) ? 'is-visible' : ''}`}
              aria-labelledby={`${slide.id}-title`}
            >
              <div className="slide__grid">
                <div className="slide__copy reveal-group">
                  <div className={`eyebrow eyebrow--${slide.accent}`}>
                    <span className="eyebrow__index">{String(pageIndex).padStart(2, '0')}</span>
                    {slide.kicker}
                  </div>
                  <h2 id={`${slide.id}-title`}>{slide.title}</h2>
                  <p className="slide__deck">{slide.deck}</p>
                  {slide.bullets && <BulletList items={slide.bullets} accent={slide.accent} />}
                  {slide.takeaway && <div className={`takeaway takeaway--${slide.accent}`}><span>↳</span>{slide.takeaway}</div>}
                </div>
                <div className="slide__visual reveal-group reveal-group--visual">
                  <Visual kind={slide.kind} />
                </div>
              </div>
              <div className="slide__footer"><span>FEDERATION_NOTES.md / {slide.kicker}</span><span>{String(pageIndex + 1).padStart(2, '0')} / 29</span></div>
            </section>
          )
        })}
      </main>
    </div>
  )
}

function BulletList({ items, accent }: { items: string[]; accent: Accent }) {
  return (
    <ul className={`bullet-list bullet-list--${accent}`}>
      {items.map((item, index) => (
          <li key={item} style={{ '--item-index': index } as CSSProperties}>
          <span className="bullet-list__dot" />
          <span>{item}</span>
        </li>
      ))}
    </ul>
  )
}

function Visual({ kind }: { kind: SlideKind }) {
  switch (kind) {
    case 'branch': return <BranchFigure />
    case 'naming': return <NamingFigure />
    case 'commits': return <CommitFigure />
    case 'why-bls': return <WhyBlsFigure />
    case 'keyset': return <KeysetFigure />
    case 'crypto': return <CryptoFigure />
    case 'derivation': return <DerivationFigure />
    case 'lifecycle': return <LifecycleFigure />
    case 'safety': return <SafetyFigure />
    case 'files': return <FilesFigure />
    case 'chapters': return <ChaptersFigure />
    case 'teaser': return <TeaserFigure />
    case 'questions': return <QuestionsFigure />
    case 'alternative': return <AlternativeFigure />
    case 'fork': return <ForkFigure />
    case 'threshold': return <ThresholdFigure />
    case 'attack': return <AttackFigure />
    case 'consensus': return <ConsensusFigure />
    case 'transport': return <TransportFigure />
    case 'dkg': return <DkgFigure />
    case 'sigall': return <SigAllFigure />
    case 'funding': return <FundingFigure />
    case 'frost': return <FrostFigure />
    case 'bdk': return <BdkFigure />
    case 'swap': return <SwapFigure />
    case 'wire': return <WireFigure />
    case 'roster': return <RosterFigure />
    case 'recovery': return <RecoveryFigure />
  }
}

function FigureFrame({
  children,
  label,
  className = '',
  footer,
}: {
  children: ReactNode
  label: string
  className?: string
  footer?: ReactNode
}) {
  return (
    <div className={`figure ${className}`}>
      <div className="figure__topline"><span>{label}</span><span className="figure__signal">● live model</span></div>
      <div className="figure__body">{children}</div>
      {footer && <div className="figure__footer">{footer}</div>}
    </div>
  )
}

function CoverFigure() {
  const members = ['M1', 'M2', 'M3', 'M4', 'M5']
  return (
    <div className="cover__figure" aria-label="Animated five member federation orbiting one aggregate BLS key">
      <div className="cover__halo cover__halo--one" />
      <div className="cover__halo cover__halo--two" />
      <div className="cover__halo cover__halo--three" />
      <div className="cover__figure-grid" />
      <div className="aggregate-key"><span className="aggregate-key__glyph">K</span><span>aggregate<br />G2 key</span></div>
      <div className="orbit orbit--outer">
        {members.map((member, index) => <span className={`orbit-node orbit-node--${index + 1}`} key={member}>{member}</span>)}
      </div>
      <div className="cover__trace cover__trace--one" />
      <div className="cover__trace cover__trace--two" />
      <div className="cover__figure-note"><span>t-of-n</span><span>one proof</span></div>
    </div>
  )
}

function BranchFigure() {
  return (
    <FigureFrame label="history / ancestry" className="branch-figure" footer={<span>merge-base <code>e2c0…</code></span>}>
      <div className="branch-map">
        <div className="branch-map__line branch-map__line--main" />
        <div className="branch-map__line branch-map__line--bls" />
        <div className="branch-map__line branch-map__line--fed" />
        <div className="branch-node branch-node--main"><span className="branch-node__dot" /><strong>main</strong><small>CDK baseline</small></div>
        <div className="branch-node branch-node--bls"><span className="branch-node__dot" /><strong>bls</strong><small>+ 10 commits</small></div>
        <div className="branch-node branch-node--fed"><span className="branch-node__dot" /><strong>bls-federation</strong><small>later layer</small></div>
        <div className="branch-map__commit branch-map__commit--one">feat: bls</div>
        <div className="branch-map__commit branch-map__commit--two">threshold / DKG</div>
        <div className="branch-map__axis"><span>single signer</span><span>shared issuer</span></div>
      </div>
    </FigureFrame>
  )
}

function NamingFigure() {
  const [selected, setSelected] = useState(2)
  const row = versionRows[selected]
  return (
    <FigureFrame label="version matrix / click a row" className="naming-figure">
      <div className="version-tabs" role="tablist" aria-label="Keyset version selector">
        {versionRows.map((version, index) => <button key={version.label} className={selected === index ? 'is-selected' : ''} type="button" onClick={() => setSelected(index)} role="tab" aria-selected={selected === index}>{version.label}</button>)}
      </div>
      <div className="version-focus">
        <div className="version-focus__label">user-facing</div>
        <div className="version-focus__value">{row.label}<span>↔</span><strong>{row.enum}</strong></div>
        <div className="version-focus__id"><span>prefix</span><code>{row.id}</code></div>
      </div>
      <div className="version-table">
        <div className="version-table__head"><span>curve</span><span>public point</span><span>meaning</span></div>
        {versionRows.map((version, index) => <button key={version.label} type="button" className={`version-table__row ${selected === index ? 'is-selected' : ''}`} onClick={() => setSelected(index)}><span>{version.curve}</span><span>{version.label === 'v3' ? 'G2 · 96 B' : 'secp · 33 B'}</span><span>{version.note}</span></button>)}
      </div>
      <div className="micro-note">token version ≠ keyset version</div>
    </FigureFrame>
  )
}

function CommitFigure() {
  const commits = [
    ['578e26e', 'feat: bls', 'types + DHKE'],
    ['59a7a80', 'rejection sampling', 'canonical Fr*'],
    ['5d4d0d7', 'batch weights', 'NUT-00 vector'],
    ['289fe9e', 'blind verification', 'e(C_, G2) = e(B, K)'],
    ['fuzz', 'valid points', 'error, not panic'],
  ]
  return (
    <FigureFrame label="hardening / commit trace" className="commit-figure" footer={<span>original <code>bls</code> branch · follow-up fixes matter</span>}>
      <div className="commit-track">
        <div className="commit-track__spine" />
        {commits.map(([hash, title, note], index) => <div className="commit-row" key={hash} style={{ '--row-index': index } as CSSProperties}><span className="commit-row__dot" /><code>{hash}</code><div><strong>{title}</strong><small>{note}</small></div></div>)}
      </div>
      <div className="commit-figure__callout"><span className="callout-mark">!</span><span>happy path → adversarial path</span></div>
    </FigureFrame>
  )
}

function WhyBlsFigure() {
  const [mode, setMode] = useState<'secp' | 'bls'>('bls')
  const secp = mode === 'secp'
  return (
    <FigureFrame label="algebra / compare the primitive" className="why-bls-figure">
      <div className="mode-toggle" role="tablist" aria-label="DHKE mode selector">
        <button type="button" className={secp ? 'is-selected' : ''} onClick={() => setMode('secp')}>secp / v1–v2</button>
        <button type="button" className={!secp ? 'is-selected' : ''} onClick={() => setMode('bls')}>BLS / v3</button>
      </div>
      <div className="algebra-stage">
        <div className="algebra-stage__curve" />
        <div className="algebra-stage__point algebra-stage__point--one">{secp ? 'Y' : 'H(s)'}</div>
        <div className="algebra-stage__point algebra-stage__point--two">B</div>
        <div className="algebra-stage__point algebra-stage__point--three">{secp ? 'C_' : 'C'}</div>
        <div className="algebra-stage__arrow algebra-stage__arrow--one" />
        <div className="algebra-stage__arrow algebra-stage__arrow--two" />
        <div className="algebra-stage__label algebra-stage__label--one">blind</div>
        <div className="algebra-stage__label algebra-stage__label--two">verify</div>
      </div>
      <div className="equation-card"><span>{secp ? 'B = Y + rG' : 'B = r · H(secret)'}</span><span>{secp ? 'C = C_ − rK' : 'e(C, G2) = e(H(secret), K)'}</span></div>
      <div className={`figure-pulse ${secp ? 'figure-pulse--secp' : ''}`}><span />{secp ? 'one mint key' : 'one public key · secret can split later'}</div>
    </FigureFrame>
  )
}

function KeysetFigure() {
  return (
    <FigureFrame label="keyset / commitment recipe" className="keyset-figure">
      <div className="keyset-id-build">
        <div className="keyset-id-build__steps"><span>amount keys</span><i>+</i><span>unit</span><i>+</i><span>fee</span><i>+</i><span>expiry</span></div>
        <div className="keyset-id-build__hash"><span>SHA-256</span><strong>02ce4c…</strong></div>
        <div className="keyset-id-build__caption">same v2 recipe · new version byte</div>
      </div>
      <div className="rotation-timeline">
        <div className="rotation-timeline__axis" />
        <div className="rotation-point rotation-point--old"><span>live</span><strong>secp keyset</strong><small>preserved</small></div>
        <div className="rotation-point rotation-point--new"><span>rotate</span><strong>BLS v3</strong><small>default for new</small></div>
        <div className="rotation-timeline__arrow">→</div>
      </div>
      <div className="keyset-badge"><span>02</span><span>v3 / Version02</span><span>G2</span></div>
    </FigureFrame>
  )
}

function CryptoFigure() {
  return (
    <FigureFrame label="types / curve boundary" className="crypto-figure">
      <div className="crypto-orbit">
        <div className="crypto-orbit__ring crypto-orbit__ring--one" />
        <div className="crypto-orbit__ring crypto-orbit__ring--two" />
        <div className="crypto-type crypto-type--g1"><strong>G1</strong><small>48 B</small><span>blind + sign</span></div>
        <div className="crypto-type crypto-type--g2"><strong>G2</strong><small>96 B</small><span>mint key</span></div>
        <div className="crypto-type crypto-type--fr"><strong>Fr*</strong><small>scalar</small><span>key / weight</span></div>
        <div className="crypto-orbit__center">e(·,·)</div>
      </div>
      <div className="crypto-legend"><span><i className="legend-dot legend-dot--violet" />point</span><span><i className="legend-dot legend-dot--cyan" />key</span><span><i className="legend-dot legend-dot--amber" />scalar</span></div>
    </FigureFrame>
  )
}

function DerivationFigure() {
  return (
    <FigureFrame label="determinism / two derivation lanes" className="derivation-figure">
      <div className="derivation-lanes">
        <div className="derivation-lane derivation-lane--wallet">
          <div className="derivation-lane__head"><span>NUT-13</span><strong>wallet secret</strong></div>
          <code>seed + keyset_id + counter</code>
          <div className="derivation-lane__node">HMAC-SHA256</div>
          <div className="derivation-lane__arrow">↓</div>
          <div className="derivation-lane__out">secret, r</div>
        </div>
        <div className="derivation-lane derivation-lane--mint">
          <div className="derivation-lane__head"><span>NUT-02</span><strong>amount key</strong></div>
          <code>i′ / attempt′</code>
          <div className="derivation-lane__node">canonical Fr*?</div>
          <div className="derivation-lane__arrow">↻ reject / retry</div>
          <div className="derivation-lane__out">kᵢ · G2</div>
        </div>
      </div>
      <div className="rejection-chip"><span>0</span><span>non-canonical</span><span>→</span><strong>next attempt</strong></div>
    </FigureFrame>
  )
}

function LifecycleFigure() {
  const [active, setActive] = useState(0)
  const step = lifecycleSteps[active]
  return (
    <FigureFrame label="proof / click a stage" className="lifecycle-figure">
      <div className="lifecycle-track">
        {lifecycleSteps.map((item, index) => <button key={item.label} type="button" className={`lifecycle-step lifecycle-step--${item.tone} ${active === index ? 'is-active' : ''}`} onClick={() => setActive(index)}><span>{item.short}</span><small>{item.label}</small></button>)}
      </div>
      <div className="lifecycle-stage">
        <div className={`lifecycle-stage__badge lifecycle-stage__badge--${step.tone}`}>{step.short}</div>
        <div className="lifecycle-stage__equation">{step.detail}</div>
        <div className="lifecycle-stage__line"><span style={{ width: `${(active + 1) / lifecycleSteps.length * 100}%` }} /></div>
        <div className="lifecycle-stage__caption">{active < 2 ? 'wallet / member' : active === 2 ? 'wallet rejects a bad mint response' : 'ordinary v3 proof path'}</div>
      </div>
      <div className="lifecycle-guard"><span>guard</span><strong>{active === 2 ? 'blind signature check' : active === 4 ? 'final proof check' : 'no DLEQ on v3'}</strong></div>
    </FigureFrame>
  )
}

function SafetyFigure() {
  return (
    <FigureFrame label="parser / context gate" className="safety-figure">
      <div className="safety-flow">
        <div className="safety-chip safety-chip--secp"><strong>33 B</strong><span>secp</span></div>
        <div className="safety-chip safety-chip--g1"><strong>48 B</strong><span>G1</span></div>
        <div className="safety-chip safety-chip--g2"><strong>96 B</strong><span>G2</span></div>
        <div className="safety-merge">PublicKey::from_slice</div>
        <div className="safety-arrow">↓</div>
        <div className="safety-gate"><span>P2PK / HTLC</span><strong>secp-only</strong></div>
        <div className="safety-result"><span>G1 / G2</span><strong>NonSecp256k1Pubkey</strong></div>
      </div>
      <div className="fuzz-strip"><span className="fuzz-strip__spark">✳</span><span>fuzz valid points</span><span>→</span><strong>error, not panic</strong></div>
    </FigureFrame>
  )
}

function FilesFigure() {
  const files = [
    ['cashu', 'nut01 / nut02 / dhke', 'types + curve math', 'violet'],
    ['cdk', 'mint + wallet', 'policy + sagas', 'cyan'],
    ['signatory', 'verify_proofs()', 'enforcement', 'amber'],
    ['fuzz', 'bls_dhke.rs', 'panic safety', 'coral'],
  ]
  return (
    <FigureFrame label="repository / vertical slice" className="files-figure">
      <div className="file-constellation">
        <div className="file-constellation__core"><span>v3</span><strong>BLS</strong><small>protocol boundary</small></div>
        {files.map(([crate, path, note, tone], index) => <div className={`file-node file-node--${tone} file-node--${index + 1}`} key={crate}><span>{crate}</span><code>{path}</code><small>{note}</small></div>)}
        <div className="file-constellation__orbit" />
      </div>
      <div className="files-footer"><code>#[cfg(v3)]</code><span>one feature · many layers</span></div>
    </FigureFrame>
  )
}

function ChaptersFigure() {
  const chapters = [
    ['01', 'why', 'pairing'],
    ['02', 'switch', 'keyset'],
    ['03', 'proof', 'lifecycle'],
    ['04', 'safety', 'curves'],
  ]
  return (
    <FigureFrame label="talk architecture / four chapters" className="chapters-figure">
      <div className="chapter-stack">
        {chapters.map(([number, title, tag], index) => <div className={`chapter-card chapter-card--${index + 1}`} key={number}><span>{number}</span><strong>{title}</strong><small>{tag}</small><i>↗</i></div>)}
      </div>
      <div className="chapter-stack__axis"><span>standalone</span><span>federation begins ↓</span></div>
    </FigureFrame>
  )
}

function TeaserFigure() {
  return (
    <FigureFrame label="federation / layers" className="teaser-figure">
      <div className="teaser-stack">
        <div className="teaser-layer teaser-layer--token"><span>ordinary token</span><strong>C</strong><small>one proof</small></div>
        <div className="teaser-layer teaser-layer--issuer"><span>threshold issuer</span><strong>kᵢ → C</strong><small>shares, then aggregate</small></div>
        <div className="teaser-layer teaser-layer--order"><span>operation ordering</span><strong>hash(envelope)</strong><small>consensus before sign</small></div>
        <div className="teaser-layer teaser-layer--treasury"><span>shared treasury</span><strong>BLS + FROST</strong><small>eCash ≠ reserves</small></div>
      </div>
      <div className="teaser-bracket"><span>federation<br />boundary</span></div>
    </FigureFrame>
  )
}

function QuestionsFigure() {
  const [resolved, setResolved] = useState<number[]>([])
  const questions = ['NUT text', 'vectors', 'live v3 demo', 'FROST appendix']
  return (
    <FigureFrame label="editorial / working set" className="questions-figure">
      <div className="question-board">
        {questions.map((question, index) => <button key={question} type="button" className={`question-card ${resolved.includes(index) ? 'is-resolved' : ''}`} onClick={() => setResolved((current) => current.includes(index) ? current.filter((item) => item !== index) : [...current, index])}><span className="question-card__pin" /><strong>{question}</strong><small>{resolved.includes(index) ? 'decided' : 'open question'}</small><i>{resolved.includes(index) ? '✓' : '?'}</i></button>)}
      </div>
      <div className="question-board__status"><span>{resolved.length} / 4 decided</span><div><span style={{ width: `${resolved.length * 25}%` }} /></div></div>
    </FigureFrame>
  )
}

function AlternativeFigure() {
  const [mode, setMode] = useState<'bundle' | 'ordinary'>('bundle')
  return (
    <FigureFrame label="design fork / bearer object" className="alternative-figure">
      <div className="alternative-toggle"><button type="button" className={mode === 'bundle' ? 'is-selected' : ''} onClick={() => setMode('bundle')}>bare secp</button><button type="button" className={mode === 'ordinary' ? 'is-selected' : ''} onClick={() => setMode('ordinary')}>BLS threshold</button></div>
      <div className={`bearer-stage bearer-stage--${mode}`}>
        {mode === 'bundle' ? <><div className="sig-bundle"><span>sig₁</span><span>sig₂</span><span>sig₃</span></div><div className="bearer-label">proof = roster + t signatures</div></> : <><div className="ordinary-proof"><span>C</span></div><div className="bearer-label">proof = one ordinary v3 C</div></>}
      </div>
      <div className="alternative-foot"><span>storage</span><strong>{mode === 'bundle' ? 'federation-aware' : 'Cashu-native'}</strong><span>verify</span><strong>{mode === 'bundle' ? 't secp checks' : 'one pairing'}</strong></div>
    </FigureFrame>
  )
}

function ForkFigure() {
  return (
    <FigureFrame label="design decision / compare" className="fork-figure">
      <div className="fork-path fork-path--left"><div className="fork-path__head"><span>secp</span><strong>bare multisig</strong></div><p>t signatures</p><p>roster in token</p><p>DLEQ × t</p><div className="fork-path__result">money knows the federation</div></div>
      <div className="fork-path fork-path--right"><div className="fork-path__head"><span>BLS</span><strong>threshold shares</strong></div><p>t shares</p><p>one aggregate K</p><p>pairing × 1</p><div className="fork-path__result">issuer knows the federation</div></div>
      <div className="fork-figure__center">same<br />consensus</div>
    </FigureFrame>
  )
}

function ThresholdFigure() {
  const [subset, setSubset] = useState<'12' | '23'>('12')
  const values = subset === '12' ? ['10B', '13B'] : ['13B', '16B']
  const lambdas = subset === '12' ? ['2', '−1'] : ['3', '−2']
  return (
    <FigureFrame label="threshold / 2-of-3 sandbox" className="threshold-figure">
      <div className="threshold-top"><span>f(x) = 7 + 3x</span><span>k = f(0) = 7</span></div>
      <svg className="threshold-plot" viewBox="0 0 620 220" role="img" aria-label="Line through three Shamir shares">
        <line className="plot-axis" x1="56" y1="180" x2="580" y2="180" />
        <line className="plot-axis" x1="56" y1="180" x2="56" y2="24" />
        <path className="plot-line" d="M56 145 L560 45" />
        <circle className="plot-origin" cx="56" cy="145" r="8" />
        <circle className={`plot-point ${subset === '12' ? 'is-hot' : ''}`} cx="206" cy="115" r="9" />
        <circle className="plot-point is-hot" cx="356" cy="85" r="9" />
        <circle className={`plot-point ${subset === '23' ? 'is-hot' : ''}`} cx="506" cy="55" r="9" />
        <text x="48" y="204">0 / k</text><text x="197" y="204">1 · 10</text><text x="345" y="204">2 · 13</text><text x="495" y="204">3 · 16</text>
      </svg>
      <div className="subset-toggle"><span>choose shares</span><button type="button" className={subset === '12' ? 'is-selected' : ''} onClick={() => setSubset('12')}>M1 + M2</button><button type="button" className={subset === '23' ? 'is-selected' : ''} onClick={() => setSubset('23')}>M2 + M3</button></div>
      <div className="threshold-calc"><span>{lambdas[0]}·{values[0]}</span><span>+</span><span>{lambdas[1]}·{values[1]}</span><span>=</span><strong>7B</strong></div>
      <div className="threshold-foot"><span>same C from any valid subset</span><code>λᵢ(0) · Cᵢ</code></div>
    </FigureFrame>
  )
}

function AttackFigure() {
  const [fixed, setFixed] = useState(false)
  const members = ['M1', 'M2', 'M3', 'M4', 'M5']
  const windows = fixed ? ['A B C', 'A B C', 'A B C', 'A B C', 'A B C'] : ['A B C', 'B C D', 'C D A', 'D A B', 'A B C']
  return (
    <FigureFrame label="security / slide the window" className="attack-figure">
      <div className="attack-toggle"><span>{fixed ? 'consensus locked' : 'naïve signing'}</span><button type="button" onClick={() => setFixed(!fixed)}>{fixed ? 'show attack' : 'lock operation'}</button></div>
      <div className="attack-grid">
        <div className="attack-grid__head"><span>member</span><span>A</span><span>B</span><span>C</span><span>D</span></div>
        {members.map((member, row) => <div className="attack-grid__row" key={member}><strong>{member}</strong>{['A', 'B', 'C', 'D'].map((letter) => <span key={letter} className={`${windows[row].includes(letter) ? 'has-share' : ''} ${fixed && windows[row].includes(letter) ? 'is-locked' : ''}`}>{windows[row].includes(letter) ? '✦' : '·'}</span>)}</div>)}
      </div>
      <div className={`attack-result ${fixed ? 'attack-result--safe' : ''}`}><span>{fixed ? 'accepted operation' : 'paid for 3 · reconstructed'}</span><strong>{fixed ? 'A B C' : 'A B C D'}</strong><small>{fixed ? 'D never receives shares' : 'unpaid D becomes spendable'}</small></div>
    </FigureFrame>
  )
}

function ConsensusFigure() {
  return (
    <FigureFrame label="consensus / apply after finality" className="consensus-figure">
      <div className="consensus-flow">
        <div className="consensus-node consensus-node--wallet"><span>wallet</span><strong>P1,P2 → A,B</strong></div>
        <div className="consensus-arrow">→</div>
        <div className="consensus-node consensus-node--envelope"><span>envelope</span><strong>hash(…)</strong><small>operation_id</small></div>
        <div className="consensus-arrow">→</div>
        <div className="consensus-node consensus-node--box"><span>AlephBFT</span><strong>one winner</strong><small>accepted order</small></div>
        <div className="consensus-arrow">→</div>
        <div className="consensus-node consensus-node--apply"><span>apply</span><strong>spend + sign</strong></div>
      </div>
      <div className="consensus-lock"><span className="lock-icon">⌁</span><span>same inputs + different outputs = conflict</span></div>
      <div className="consensus-members"><span>M1</span><span>M2</span><span>M3</span><span>M4</span><span>M5</span><i>→ shares only for A, B</i></div>
    </FigureFrame>
  )
}

function TransportFigure() {
  const [plane, setPlane] = useState<'public' | 'private'>('public')
  const memberPositions = [[85, 48], [264, 20], [424, 64], [334, 178], [126, 174]]
  return (
    <FigureFrame label="network graph / click a plane" className="transport-figure">
      <div className="transport-toggle"><button type="button" className={plane === 'public' ? 'is-selected' : ''} onClick={() => setPlane('public')}>public / fan-out</button><button type="button" className={plane === 'private' ? 'is-selected' : ''} onClick={() => setPlane('private')}>private / mesh</button></div>
      <svg className={`transport-graph transport-graph--${plane}`} viewBox="0 0 620 250" role="img" aria-label={`${plane} federation transport graph`}>
        {plane === 'public' ? <>
          {memberPositions.map(([x, y], index) => <line key={`public-${index}`} className="transport-edge transport-edge--fan" x1="310" y1="125" x2={x} y2={y} />)}
          {memberPositions.map(([x, y], index) => <circle key={`packet-${index}`} className="transport-packet" r="4"><animateMotion dur={`${2.6 + index * 0.18}s`} repeatCount="indefinite" path={`M310,125 L${x},${y}`} /></circle>)}
          <circle className="transport-wallet" cx="310" cy="125" r="25" /><text className="transport-label transport-label--wallet" x="310" y="130">wallet</text>
        </> : <>
          {memberPositions.map(([x, y], index) => { const next = memberPositions[(index + 1) % memberPositions.length]; return <line key={`mesh-${index}`} className="transport-edge transport-edge--mesh" x1={x} y1={y} x2={next[0]} y2={next[1]} /> })}
          {memberPositions.map(([x, y], index) => <circle key={`mesh-packet-${index}`} className="transport-packet transport-packet--mesh" r="4"><animateMotion dur={`${2.2 + index * 0.14}s`} repeatCount="indefinite" path={`M${x},${y} L${memberPositions[(index + 1) % memberPositions.length][0]},${memberPositions[(index + 1) % memberPositions.length][1]}`} /></circle>)}
          <path className="transport-edge transport-edge--mesh transport-edge--cross" d="M85 48 L334 178 M264 20 L126 174 M424 64 L126 174" />
        </>}
        {memberPositions.map(([x, y], index) => <g key={`member-${index}`}><circle className="transport-member" cx={x} cy={y} r="19" /><text className="transport-label" x={x} y={y + 4}>{`M${index + 1}`}</text></g>)}
      </svg>
      <div className="transport-legend"><span><i className="legend-dot legend-dot--cyan" />{plane === 'public' ? 'same Cashu request' : 'authenticated envelope'}</span><span><i className="legend-dot legend-dot--amber" />{plane === 'public' ? 'threshold shares' : 'consensus + DKG'}</span></div>
    </FigureFrame>
  )
}

function DkgFigure() {
  const [step, setStep] = useState(0)
  const current = dkgSteps[step]
  const members = ['M1', 'M2', 'M3']
  return (
    <FigureFrame label="DKG / step through the ceremony" className="dkg-figure">
      <div className="dkg-stepper">{dkgSteps.map((item, index) => <button key={item.label} type="button" className={step === index ? 'is-selected' : ''} onClick={() => setStep(index)}><span>0{index + 1}</span>{item.label}</button>)}</div>
      <svg className={`dkg-graph dkg-graph--step-${step}`} viewBox="0 0 620 250" role="img" aria-label="Three member BLS DKG data flow">
        <path className="dkg-path dkg-path--top" d="M90 70 C220 0 400 0 530 70" />
        <path className="dkg-path dkg-path--mid" d="M90 70 C210 160 410 160 530 70" />
        <path className="dkg-path dkg-path--bottom" d="M90 70 C200 250 420 250 530 70" />
        {members.map((member, index) => <g key={member}><circle className="dkg-member" cx={90 + index * 220} cy="70" r="27" /><text className="dkg-label" x={90 + index * 220} y="75">{member}</text><text className="dkg-sub" x={90 + index * 220} y="122">{index === 0 ? 'f₁(x)' : index === 1 ? 'f₂(x)' : 'f₃(x)'}</text></g>)}
        {step >= 1 && <><circle className="dkg-packet dkg-packet--a" r="5"><animateMotion dur="2.4s" repeatCount="indefinite" path="M90,70 C220,0 400,0 530,70" /></circle><circle className="dkg-packet dkg-packet--b" r="5"><animateMotion dur="2.8s" repeatCount="indefinite" path="M530,70 C410,160 210,160 90,70" /></circle></>}
        {step >= 2 && <><circle className="dkg-packet dkg-packet--c" r="5"><animateMotion dur="2.2s" repeatCount="indefinite" path="M90,70 C200,250 420,250 530,70" /></circle><circle className="dkg-packet dkg-packet--d" r="5"><animateMotion dur="2.5s" repeatCount="indefinite" path="M530,70 C420,250 200,250 90,70" /></circle></>}
        {step >= 3 && <circle className="dkg-sum" cx="310" cy="188" r="31" />}
        {step >= 3 && <text className="dkg-sum-label" x="310" y="194">Σ kᵢ</text>}
        {step >= 4 && <><circle className="dkg-active" cx="310" cy="188" r="45" /><text className="dkg-active-label" x="310" y="194">K / G2</text></>}
      </svg>
      <div className="dkg-current"><span>{current.code}</span><strong>{current.detail}</strong><small>{current.label} · {current.code === 'sealed transcript' ? 'nobody adapts to later reveals' : current.code === 'public commitments' ? 'checkable in G2' : current.code === 'private evaluation' ? 'per receiver, authenticated' : current.code === 'verified share' ? 'ready for threshold issuance' : 'activate only after transcript signatures'}</small></div>
    </FigureFrame>
  )
}

function SigAllFigure() {
  const [bound, setBound] = useState(false)
  return (
    <FigureFrame label="security / spend intent" className="sigall-figure">
      <div className="sigall-toggle"><span>{bound ? 'SIG_ALL bound' : 'bearer proof exposed'}</span><button type="button" onClick={() => setBound(!bound)}>{bound ? 'show the hole' : 'bind the spend'}</button></div>
      <div className="sigall-stage">
        <div className="sigall-proof"><span>P1 + P2</span><small>raw proofs</small></div>
        <div className="sigall-arrow">→</div>
        <div className="sigall-outputs"><span className={bound ? 'is-bound' : ''}>{bound ? 'A · B' : 'A · B'}</span><small>user outputs</small></div>
        <div className="sigall-arrow">→</div>
        <div className={`sigall-copy ${bound ? 'is-safe' : ''}`}><span>{bound ? 'same bytes' : 'evil member rewrites'}</span><strong>{bound ? 'A · B' : 'X · Y'}</strong><small>{bound ? 'SIG_ALL verifies' : 'races consensus'}</small></div>
      </div>
      <div className={`sigall-result ${bound ? 'sigall-result--safe' : ''}`}><span>{bound ? 'owner intent survives' : 'critical open hole'}</span><strong>{bound ? 'mutated operation rejected' : 'valid shares · wrong outputs'}</strong></div>
    </FigureFrame>
  )
}

function FundingFigure() {
  return (
    <FigureFrame label="treasury / split the authority" className="funding-figure">
      <div className="funding-columns">
        <div className="funding-column funding-column--cashu"><div className="funding-column__head"><span>eCash issuer</span><strong>BLS</strong></div><div className="funding-column__flow"><span>B = r · H(s)</span><i>→</i><span>kᵢ · B</span><i>→</i><span>C</span></div><small>blind shares / proof</small></div>
        <div className="funding-column funding-column--treasury"><div className="funding-column__head"><span>funding backend</span><strong>FROST</strong></div><div className="funding-column__flow"><span>observe</span><i>→</i><span>intent</span><i>→</i><span>sign</span></div><small>Bitcoin / Lightning custody</small></div>
      </div>
      <div className="funding-quorum"><span>same roster</span><strong>same economic quorum</strong><span>separate keys</span></div>
    </FigureFrame>
  )
}

function FrostFigure() {
  return (
    <FigureFrame label="treasury / dual ceremony" className="frost-figure">
      <div className="dual-ceremony">
        <div className="ceremony ceremony--bls"><span className="ceremony__curve">G2</span><strong>BLS DKG</strong><small>Cashu keyset</small><div className="ceremony__steps"><span>commit</span><span>share</span><span>activate</span></div></div>
        <div className="ceremony__split">≠</div>
        <div className="ceremony ceremony--frost"><span className="ceremony__curve">secp</span><strong>FROST DKG</strong><small>treasury root</small><div className="ceremony__steps"><span>nonce</span><span>round</span><span>sign</span></div></div>
      </div>
      <div className="frost-note"><span>same member roster</span><strong>different key hierarchy</strong></div>
    </FigureFrame>
  )
}

function BdkFigure() {
  const [mode, setMode] = useState<'mint' | 'melt'>('mint')
  const mintSteps = ['quote', 'address', 'observe', 'quorum paid', 'issue']
  const meltSteps = ['quote', 'spend proofs', 'intent', 'FROST sign', 'broadcast']
  const steps = mode === 'mint' ? mintSteps : meltSteps
  return (
    <FigureFrame label="BDK / click mint or melt" className="bdk-figure">
      <div className="bdk-toggle"><button type="button" className={mode === 'mint' ? 'is-selected' : ''} onClick={() => setMode('mint')}>mint</button><button type="button" className={mode === 'melt' ? 'is-selected' : ''} onClick={() => setMode('melt')}>melt</button></div>
      <div className="bdk-timeline">{steps.map((item, index) => <div className="bdk-step" key={item}><span>{String(index + 1).padStart(2, '0')}</span><strong>{item}</strong>{index < steps.length - 1 && <i>→</i>}</div>)}</div>
      <div className={`bdk-terminal bdk-terminal--${mode}`}><span>{mode === 'mint' ? 'reserve observation' : 'payout intent'}</span><code>{mode === 'mint' ? 'Q = C = 4' : 'signed bytes → broadcast'}</code><strong>{mode === 'mint' ? 'eCash can issue' : 'funds can move'}</strong></div>
    </FigureFrame>
  )
}

function SwapFigure() {
  const [step, setStep] = useState(0)
  const steps = [
    ['fan-out', 'wallet → M1…M5', 'same ordinary Cashu request'],
    ['envelope', 'hash(inputs + outputs)', 'one operation_id'],
    ['finalize', 'AlephBFT → accepted', 'spend once'],
    ['aggregate', 'C₁ + C₂ + C₃', 'ordinary proof'],
  ]
  const current = steps[step]
  return (
    <FigureFrame label="swap / click the timeline" className="swap-figure">
      <div className="swap-steps">{steps.map((item, index) => <button type="button" key={item[0]} className={step === index ? 'is-selected' : ''} onClick={() => setStep(index)}><span>0{index + 1}</span><strong>{item[0]}</strong></button>)}</div>
      <div className="swap-stage"><div className={`swap-stage__icon swap-stage__icon--${step}`}><span>{step === 0 ? '★' : step === 1 ? '⌘' : step === 2 ? '✓' : 'C'}</span></div><div className="swap-stage__equation"><strong>{current[1]}</strong><small>{current[2]}</small></div></div>
      <div className="swap-footer"><span>inputs</span><code>P1 · P2</code><span>outputs</span><code>A · B</code><span className="swap-footer__lock">⌁ once</span></div>
    </FigureFrame>
  )
}

function WireFigure() {
  const [kind, setKind] = useState('Swap')
  const operation = operationKinds.find((item) => item.label === kind) ?? operationKinds[0]
  return (
    <FigureFrame label="AlephBFT / operation catalog" className="wire-figure">
      <div className="wire-tabs">{operationKinds.map((item) => <button type="button" className={kind === item.label ? 'is-selected' : ''} key={item.label} onClick={() => setKind(item.label)}>{item.label}</button>)}</div>
      <div className="wire-stream"><span className="wire-stream__origin">wallet</span><div className="wire-stream__line"><span className="wire-stream__packet" /></div><div className={`wire-stream__event wire-stream__event--${operation.tone}`}><span>ordered</span><strong>{operation.label}</strong><code>{operation.count}</code></div><div className="wire-stream__line wire-stream__line--short"><span className="wire-stream__packet wire-stream__packet--two" /></div><span className="wire-stream__tail">apply</span></div>
      <div className="wire-inventory"><span>in log</span><strong>named envelope</strong><span>off log</span><strong>BLS / FROST / HTTP</strong></div>
    </FigureFrame>
  )
}

function RosterFigure() {
  const fields = ['setup auth', 'thresholds', 'member IDs', 'public URLs', 'identity keys']
  const digest = ['wallet version', 'payment policy', 'checkpoint', 'FROST policy']
  return (
    <FigureFrame label="genesis / what gets hashed" className="roster-figure">
      <div className="roster-hash"><div className="roster-hash__column"><span>federation_id</span>{fields.map((field) => <code key={field}>{field}</code>)}<strong>SHA-256</strong></div><div className="roster-hash__arrow">→</div><div className="roster-hash__result"><span>federation_id</span><strong>new mint</strong><small>change a URL → new ID</small></div></div>
      <div className="roster-digest"><span>config_digest also binds</span>{digest.map((field) => <code key={field}>{field}</code>)}<b>keyset rotation is the live exception</b></div>
    </FigureFrame>
  )
}

function RecoveryFigure() {
  const [scenario, setScenario] = useState<'db' | 'lag' | 'secret'>('db')
  const scenarios = {
    db: ['DB lost · secrets intact', 'genesis replay / catch-up', 'member can return after digest audit'],
    lag: ['network lag / stale checkpoint', 'peer catch-up + suffix verify', 'fail closed until ready'],
    secret: ['identity or BLS share lost', 'no peer reconstruction', 'seat is dead · new federation'],
  }
  const active = scenarios[scenario]
  return (
    <FigureFrame label="operations / restore cookbook" className="recovery-figure">
      <div className="recovery-tabs">{Object.entries(scenarios).map(([key, value]) => <button key={key} type="button" className={scenario === key ? 'is-selected' : ''} onClick={() => setScenario(key as keyof typeof scenarios)}>{value[0]}</button>)}</div>
      <div className="recovery-stack"><div className="recovery-layer recovery-layer--book"><span>the book</span><strong>journal · checkpoints · state digest</strong></div><div className="recovery-layer recovery-layer--pen"><span>the pen</span><strong>identity · BLS · FROST</strong></div></div>
      <div className={`recovery-result recovery-result--${scenario}`}><span>{active[0]}</span><strong>{active[1]}</strong><small>{active[2]}</small></div>
    </FigureFrame>
  )
}

export default App
