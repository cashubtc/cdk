import { DeckSvg, SvgNode, SvgEdge, FlowParticles, G1, G2, SECP, INK, DIM, FAINT, type XY } from './graph.tsx'
import { useOnceInView } from '../primitives.tsx'

function EqBox({
  p,
  w,
  label,
  sub,
  color,
}: {
  p: XY
  w: number
  label: string
  sub?: string
  color?: string
}) {
  return (
    <g>
      <rect
        x={p.x - w / 2}
        y={p.y - 14}
        width={w}
        height={28}
        rx={7}
        fill="#141822"
        stroke={color ?? 'rgba(234,231,224,0.18)'}
        strokeWidth={1.2}
      />
      <text x={p.x} y={p.y + 1} textAnchor="middle" fontSize={10.5} fill={INK} fontFamily="JetBrains Mono" fontWeight={500}>
        {label}
      </text>
      {sub && (
        <text x={p.x} y={p.y + 13} textAnchor="middle" fontSize={7.5} fill={DIM} fontFamily="JetBrains Mono">
          {sub}
        </text>
      )}
    </g>
  )
}

export function Duel() {
  const { ref, inView } = useOnceInView<HTMLDivElement>()
  return (
    <div ref={ref} className="figure">
      <DeckSvg width={960} height={360}>
        {/* divider */}
        <line x1={470} y1={36} x2={470} y2={318} stroke="rgba(234,231,224,0.1)" strokeWidth={1} strokeDasharray="3 5" />

        {/* secp column */}
        <text x={150} y={28} textAnchor="middle" fontSize={10.5} fontWeight={700} fill={SECP} fontFamily="JetBrains Mono">
          secp256k1 · additive
        </text>
        <EqBox p={{ x: 150, y: 62 }} w={248} label="Y = H(secret)" sub="v1 / v2, old cashu" color={SECP} />
        <EqBox p={{ x: 150, y: 110 }} w={248} label="B_ = Y + rG" sub="blind" color={SECP} />
        <EqBox p={{ x: 150, y: 158 }} w={248} label="C_ = k · B_" sub="sign with one k" color={SECP} />
        <EqBox p={{ x: 150, y: 206 }} w={248} label="C = C_ − rK" sub="unblind" color={SECP} />
        <SvgNode p={{ x: 150, y: 268 }} label="k" state="secp" r={15} pulse />
        <SvgNode p={{ x: 336, y: 268 }} label="DLEQ" state="dim" r={15} />
        <SvgEdge a={{ x: 150, y: 268 }} b={{ x: 336, y: 268 }} color={SECP} dashed width={1.4} />
        <text x={150} y={304} textAnchor="middle" fontSize={8.5} fill={FAINT} fontFamily="JetBrains Mono">
          one mint holds k — DLEQ proves the key
        </text>

        {/* BLS column */}
        <text x={710} y={28} textAnchor="middle" fontSize={10.5} fontWeight={700} fill={G2} fontFamily="JetBrains Mono">
          BLS12-381 · pairing
        </text>
        <EqBox p={{ x: 650, y: 62 }} w={248} label="H(secret) → G1" sub="hash-to-curve, DST-bound" color={G2} />
        <EqBox p={{ x: 650, y: 110 }} w={248} label="B = r · H(secret)" sub="blind in G1" color={G1} />
        <EqBox p={{ x: 650, y: 158 }} w={248} label="C_ = k · B" sub="sign — member share later" color={G1} />
        <EqBox p={{ x: 650, y: 206 }} w={248} label="C = r⁻¹ · C_" sub="unblind" color={G1} />
        <SvgNode p={{ x: 650, y: 268 }} label="K" state="g2" r={16} pulse />
        <SvgNode p={{ x: 822, y: 268 }} label="k?" state="dim" r={16} />
        <SvgEdge a={{ x: 650, y: 268 }} b={{ x: 822, y: 268 }} color={G2} width={1.6} animated={inView} />
        {inView && <FlowParticles path="M 650 268 L 822 268" n={3} color={G2} size={2.4} dur={1.2} />}
        <text x={650} y={304} textAnchor="middle" fontSize={8.5} fill={FAINT} fontFamily="JetBrains Mono">
          e(C, G2) == e(H(secret), K) — no k, ever
        </text>
      </DeckSvg>
      <div className="figcap">
        <span className="dot" />
        <span>
          the pairing equation is why federation can later issue threshold shares — a wallet checks one signature
          against the aggregate K and never needs the private scalar.
        </span>
      </div>
    </div>
  )
}