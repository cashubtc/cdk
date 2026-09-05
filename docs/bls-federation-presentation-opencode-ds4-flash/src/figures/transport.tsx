import { useState } from 'react'
import { DeckSvg, G1, G2, DIM, FAINT, INK, FlowParticles, SvgEdge, SvgNode, ring, type XY } from './graph.tsx'
import { useOnceInView } from '../primitives.tsx'

const M = ['M1', 'M2', 'M3', 'M4', 'M5']

export function Transport() {
  const { ref, inView } = useOnceInView<HTMLDivElement>()
  const [t, setT] = useState<'http' | 'iroh'>('http')

  const pub: XY[] = ring(230, 235, 118, 5, -Math.PI / 2)
  const priv: XY[] = ring(742, 235, 118, 5, Math.PI / 2 + 0.2)
  const wallet: XY = { x: 230, y: 235 }

  const tunnel = t === 'http' ? 'https://mint-a.example' : 'iroh://<endpoint-id>'

  return (
    <div ref={ref} className="figure">
      <DeckSvg width={960} height={470}>
        {/* divider */}
        <line x1={470} y1={30} x2={470} y2={430} stroke="rgba(234,231,224,0.1)" strokeWidth={1} strokeDasharray="3 5" />

        {/* ============ public plane ============ */}
        <text x={120} y={40} textAnchor="middle" fontSize={11} fontWeight={700} fill={G1} fontFamily="JetBrains Mono">
          public · wallet fan-out
        </text>
        {pub.map((m, i) => (
          <SvgEdge key={`ps${i}`} a={wallet} b={m} color="rgba(167,139,250,0.45)" width={1.8} animated={inView} />
        ))}
        {inView &&
          pub.map((m, i) => (
            <FlowParticles key={`pf${i}`} path={`M ${wallet.x} ${wallet.y} L ${m.x} ${m.y}`} n={1} color={G1} size={2.2} dur={1.2} begin={i * 0.1} />
          ))}
        {inView &&
          pub.map((m, i) => (
            <FlowParticles key={`pb${i}`} path={`M ${m.x} ${m.y} L ${wallet.x} ${wallet.y}`} n={1} color={G2} size={2.2} dur={1.2} begin={0.4 + i * 0.12} />
          ))}
        <SvgNode p={wallet} r={15} label="W" state="idle" pulse />
        {pub.map((m, i) => (
          <SvgNode key={`pn${i}`} p={m} label={M[i]} state="idle" />
        ))}
        <text x={230} y={120} textAnchor="middle" fontSize={8} fill={FAINT} fontFamily="JetBrains Mono">
          same Cashu request to every member
        </text>
        <text x={230} y={358} textAnchor="middle" fontSize={8} fill={FAINT} fontFamily="JetBrains Mono">
          wallet keeps any T valid shares → one proof
        </text>

        {/* ============ private plane ============ */}
        <text x={742} y={40} textAnchor="middle" fontSize={11} fontWeight={700} fill={DIM} fontFamily="JetBrains Mono">
          private · member mesh
        </text>
        {priv.map((m, i) => {
          const nxt = priv[(i + 1) % priv.length]
          return <SvgEdge key={`pme${i}`} a={m} b={nxt} color="rgba(234,231,224,0.2)" width={1.5} dashed animated={inView} />
        })}
        {inView && (
          <FlowParticles
            path={priv
              .map((m, i) => {
                const nxt = priv[(i + 1) % priv.length]
                return `M ${m.x} ${m.y} L ${nxt.x} ${nxt.y}`
              })
              .join(' ')}
            n={4}
            color={DIM}
            size={1.9}
            dur={3}
            begin={0.3}
          />
        )}
        {priv.map((m, i) => (
          <SvgNode key={`prn${i}`} p={m} label={M[i]} state="idle" />
        ))}
        <text x={742} y={120} textAnchor="middle" fontSize={8} fill={FAINT} fontFamily="JetBrains Mono">
          submit envelope · AlephBFT · journal catch-up
        </text>
        <text x={742} y={358} textAnchor="middle" fontSize={8} fill={FAINT} fontFamily="JetBrains Mono">
          /federation/v1 · member identity key auth
        </text>

        {/* transport tunnel label */}
        <rect x={375} y={380} width={210} height={34} rx={17} fill="#141822" stroke={t === 'http' ? G2 : 'rgba(234,231,224,0.3)'} strokeWidth={1.5} />
        <text x={480} y={401} textAnchor="middle" fontSize={9} fontWeight={600} fill={INK} fontFamily="JetBrains Mono">
          {tunnel}
        </text>
      </DeckSvg>

      <div className="lagrange-btns">
        <button type="button" className={`btn ${t === 'http' ? 'on' : ''}`} onClick={() => setT('http')}>
          http
        </button>
        <button type="button" className={`btn ${t === 'iroh' ? 'on' : ''}`} onClick={() => setT('iroh')}>
          iroh
        </button>
        <span className="figtoast">
          {t === 'http'
            ? 'the same two planes over TCP: public_mint_url for wallets, federation_api_url for peers.'
            : 'the same routes tunneled: iroh://<endpoint-id> for both. iroh is the pipe, not a new protocol — and it can be mixed per plane.'}
        </span>
      </div>
      <div className="figcap" style={{ marginTop: '0.5rem' }}>
        <span className="dot" />
        <span>
          two networks, one federation: the star is wallet ↔ members (shares back), the mesh is members ↔ members
          (agreement first). the split that matters is not http vs iroh — it is public fan-out vs private mesh.
        </span>
      </div>
    </div>
  )
}