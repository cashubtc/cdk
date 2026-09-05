import { PageHead, Reveal, Tag, Callout } from '../primitives.tsx'

export default function Layers() {
  return (
    <>
      <PageHead
        idx="02"
        section="the story so far"
        title={
          <>
            Two layers, <span className="cd">one checkout</span>
          </>
        }
        lead={
          <>
            This deck covers <b>main → bls → bls-federation</b>. They are not one story. The original{' '}
            <span className="math">bls</span> branch is a small single-signer BLS integration. Everything this
            presentation spends most of its time on — threshold shares, DKG, consensus, rosters — lives on the much
            larger <span className="math">bls-federation</span> layer above it.
          </>
        }
      />
      <Reveal delay={0.1}>
        <div className="layer-map">
          <div className="layer">
            <Tag c="cs">main</Tag>
            <span className="layer-name">standalone CDK baseline</span>
            <span className="layer-arrow">▶</span>
          </div>
          <div className="layer">
            <Tag c="c1">bls</Tag>
            <span className="layer-name">
              original BLS integration — <span className="math">~47 files, +2463 / −272</span>, 10 commits above main
            </span>
            <span className="layer-arrow">▶</span>
          </div>
          <div className="layer">
            <Tag c="c1">bls-federation</Tag>
            <span className="layer-name">
              threshold issuance, consensus, DKG, transport — <span className="math">1000+ commits</span>
            </span>
            <span className="layer-arrow dim">★</span>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.16}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-h">on <span className="mono" style={{ color: 'var(--g1)' }}>bls</span></h3>
            <ul className="col-list">
              <li>BLS12-381 types, versioned DHKE</li>
              <li>v3 keyset IDs, blind signature verification</li>
              <li>still single-signer — no threshold anywhere</li>
              <li>merge-base: <span className="math">e2c0d0a4</span></li>
            </ul>
          </div>
          <div className="col">
            <h3 className="col-title">on <span className="mono" style={{ color: 'var(--g2)' }}>bls-federation</span></h3>
            <ul className="col-list">
              <li><span className="math">BlsSignerId</span>, <span className="math">BlsSecretKeyShare</span>, threshold keysets</li>
              <li>consensus-before-sign, mix-and-match defense</li>
              <li>BLS DKG, FROST DKG, federated BDK / Bark</li>
              <li>merge-base: <span className="math">eb34616e</span> (<span className="math">origin/bls</span>)</li>
            </ul>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.22}>
        <Callout k="do not mix these">
          the single-signer branch and the federation layer are different stories with different cast and stakes.
          keep them apart, or the “why BLS” motivation and the “how a federation works” machinery blur into one another.
        </Callout>
      </Reveal>
    </>
  )
}