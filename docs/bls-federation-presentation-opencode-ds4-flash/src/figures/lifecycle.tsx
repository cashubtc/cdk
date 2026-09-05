import { DeckSvg, G1, G2, OK, ROSE, DIM, FAINT, FlowParticles, type XY } from './graph.tsx'
import { useOnceInView } from '../primitives.tsx'

const stages: { p: XY; label: string; sub: string; color: string; w: number }[] = [
  { p: { x: 96, y: 78 }, label: 'wallet', sub: 'derive secret, r', color: 'rgba(234,231,224,0.3)', w: 120 },
  { p: { x: 288, y: 78 }, label: 'blind', sub: 'B = r · H(secret)', color: G1, w: 150 },
  { p: { x: 468, y: 78 }, label: 'mint · sign', sub: 'C_ = k · B', color: G1, w: 130 },
  { p: { x: 668, y: 78 }, label: 'blind check', sub: 'e(C_,G2) == e(B,K)', color: OK, w: 165 },
  { p: { x: 856, y: 78 }, label: 'unblind', sub: 'C = r⁻¹ · C_', color: G1, w: 128 },
]

const paths = [
  'M 96 78 L 288 78',
  'M 288 78 L 468 78',
  'M 468 78 L 668 78',
  'M 668 78 L 856 78',
]

export function Lifecycle() {
  const { ref, inView } = useOnceInView<HTMLDivElement>()
  return (
    <div ref={ref} className="figure">
      <DeckSvg width={960} height={190}>
        {stages.map((s, i) => (
          <g key={i}>
            <rect x={s.p.x - s.w / 2} y={s.p.y - 15} width={s.w} height={30} rx={8} fill="#141822" stroke={s.color} strokeWidth={1.5} />
            <text x={s.p.x} y={s.p.y} textAnchor="middle" fontSize={9.5} fontWeight={700} fill="#eae7e0" fontFamily="JetBrains Mono">
              {s.label}
            </text>
            <text x={s.p.x} y={s.p.y + 13} textAnchor="middle" fontSize={8} fill={DIM} fontFamily="JetBrains Mono">
              {s.sub}
            </text>
          </g>
        ))}
        {/* proof check happens off-figure, then the final pairing */}
        <g>
          <rect x={668} y={138} width={292} height={30} rx={7} fill="#141822" stroke={OK} strokeWidth={1.5} />
          <text x={814} y={153} textAnchor="middle" fontSize={9.5} fontWeight={700} fill="#eae7e0" fontFamily="JetBrains Mono">
            proof check
          </text>
          <text x={814} y={166} textAnchor="middle" fontSize={7} fill={DIM} fontFamily="JetBrains Mono">
            e(C, G2) == e(H(secret), K)
          </text>
        </g>
        {inView && paths.map((p, i) => <FlowParticles key={i} path={p} n={3} color={i === 0 ? G2 : G1} size={2.4} dur={1.2} begin={i * 0.28} />)}
        {inView && <FlowParticles path="M 668 138 L 856 138" n={2} color={G1} size={2.4} dur={1.2} />}

        {/* no DLEQ marker */}
        <text x={96} y={118} textAnchor="middle" fontSize={8} fill={ROSE} fontFamily="JetBrains Mono">
          DLEQ forbidden on v3
        </text>
        <text x={468} y={118} textAnchor="middle" fontSize={8} fill={FAINT} fontFamily="JetBrains Mono">
          only k · B, no extra proof
        </text>
        <text x={668} y={118} textAnchor="middle" fontSize={8} fill={FAINT} fontFamily="JetBrains Mono">
          blind check added by fix 289fe9e44
        </text>
      </DeckSvg>
      <div className="figcap">
        <span className="dot" />
        <span>
          the pipeline the walk makes obvious: derive → blind → sign → blind-check → unblind → proof-check.
          pairing checks both directions; the token itself never knows about federation.
        </span>
      </div>
    </div>
  )
}