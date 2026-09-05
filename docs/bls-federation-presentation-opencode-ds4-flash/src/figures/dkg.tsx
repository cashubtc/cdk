import { useEffect, useState } from 'react'
import { DeckSvg, G1, G2, INK, DIM, FAINT, OK, PANEL, SvgEdge, SvgNode, FlowParticles, type XY } from './graph.tsx'
import { useOnceInView } from '../primitives.tsx'

const PHASES = [
  'agree params',
  'pick f_j, seal',
  'hash-commit',
  'reveal',
  'private delivery',
  'sum → K',
  'transcript + activate',
]

/* tiny 3-member, 2-of-3 example from the notes */
const POLY = ['f1 = 4 + x', 'f2 = 2 + 5x', 'f3 = 1 + 2x']
const COMMIT = ['C1 = [4·G2, 1·G2]', 'C2 = [2·G2, 5·G2]', 'C3 = [1·G2, 2·G2]']
/* f_j(i) delivered from j to i */
const DELIVER: Record<string, number> = {
  '1>2': 6,
  '1>3': 7,
  '2>1': 7,
  '2>3': 17,
  '3>1': 3,
  '3>2': 5,
}
const SUM = ['k1 = 5+7+3 = 15', 'k2 = 6+12+5 = 23', 'k3 = 7+17+7 = 31']

const NODES: XY[] = [
  { x: 245, y: 110 },
  { x: 715, y: 110 },
  { x: 480, y: 350 },
]

/* keyframes per phase */
function phaseStep(phase: number): number {
  switch (phase) {
    case 1:
      return 1
    case 2:
      return 2
    case 3:
      return 3
    case 4:
      return 4
    default:
      return 5
  }
}

export function Dkg() {
  const { ref, inView } = useOnceInView<HTMLDivElement>()
  const [phase, setPhase] = useState(0)
  const [playId, setPlayId] = useState(0)

  useEffect(() => {
    if (!inView || playId === 0) return
    const timers: ReturnType<typeof setTimeout>[] = []
    ;[1, 2, 3, 4, 5, 6].forEach((p, i) => {
      timers.push(setTimeout(() => setPhase(p), 350 + i * 1450))
    })
    return () => timers.forEach(clearTimeout)
  }, [playId, inView])

  const replay = () => {
    setPhase(0)
    setPlayId((x) => x + 1)
  }

  const st = phaseStep(phase)

  return (
    <div ref={ref} className="figure">
      <DeckSvg width={960} height={430}>
        {/* member triangle + private edges */}
        {[0, 1, 2].map((i) => {
          const j = (i + 1) % 3
          return <SvgEdge key={i} a={NODES[i]} b={NODES[j]} color="rgba(234,231,224,0.14)" dashed width={1.3} animated={inView} />
        })}

        {/* members */}
        {NODES.map((m, i) => (
          <g key={i}>
            <SvgNode p={m} label={`M${i + 1}`} r={16} state={st >= 4 ? 'g1' : st >= 1 ? 'g2' : 'idle'} />
            <text x={m.x} y={m.y + 32} textAnchor="middle" fontSize={8.5} fill={DIM} fontFamily="JetBrains Mono">
              {st >= 1 ? POLY[i] : 'private polynomial'}
            </text>
            {st >= 2 && (
              <text x={m.x} y={m.y + 46} textAnchor="middle" fontSize={7.5} fill={FAINT} fontFamily="JetBrains Mono">
                H(my reveal) — cannot adapt
              </text>
            )}
            {st >= 3 && (
              <text x={m.x} y={m.y + 46} textAnchor="middle" fontSize={7.5} fill={G2} fontFamily="JetBrains Mono">
                {COMMIT[i]}
              </text>
            )}
            {st >= 5 && (
              <text x={m.x} y={m.y + 60} textAnchor="middle" fontSize={8.5} fontWeight={700} fill={OK} fontFamily="JetBrains Mono">
                {SUM[i]}
              </text>
            )}
          </g>
        ))}

        {/* private delivery arrows */}
        {st >= 4 && (
          <>
            {Object.entries(DELIVER).map(([k, v]) => {
              const [a, b] = k.split('>').map((x) => Number(x) - 1)
              const from = NODES[a]
              const to = NODES[b]
              const mx = (from.x + to.x) / 2
              const my = (from.y + to.y) / 2
              return (
                <g key={k}>
                  <SvgEdge a={from} b={to} color={G1} width={1.5} animated />
                  <text x={mx} y={my - 6} textAnchor="middle" fontSize={8} fill={G1} fontFamily="JetBrains Mono">
                    f{a + 1}({b + 1}) = {v}
                  </text>
                </g>
              )
            })}
          </>
        )}
        {inView && st >= 4 && (
          <>
            {Object.keys(DELIVER).map((k, i) => {
              const [a, b] = k.split('>').map((x) => Number(x) - 1)
              return (
                <FlowParticles
                  key={k}
                  path={`M ${NODES[a].x} ${NODES[a].y} L ${NODES[b].x} ${NODES[b].y}`}
                  n={1}
                  color={G1}
                  size={2}
                  dur={1.1}
                  begin={i * 0.25}
                />
              )
            })}
          </>
        )}

        {/* aggregate */}
        <g>
          <rect x={382} y={205} width={196} height={64} rx={12} fill={PANEL} stroke={st >= 5 ? G2 : 'rgba(234,231,224,0.2)'} strokeWidth={1.5} />
          <text x={480} y={230} textAnchor="middle" fontSize={10} fontWeight={700} fill={st >= 5 ? G2 : FAINT} fontFamily="JetBrains Mono">
            joint f(x) = 7 + 8x
          </text>
          <text x={480} y={248} textAnchor="middle" fontSize={9} fill={INK} fontFamily="JetBrains Mono">
            K = Σ Cⱼ[0] = 7·G2
          </text>
        </g>

        {/* phase chips */}
        <g>
          {PHASES.map((p, i) => {
            const on = i === phase
            const past = i < phase
            return (
              <g key={p}>
                <rect
                  x={18 + i * 138}
                  y={418}
                  width={132}
                  height={22}
                  rx={11}
                  fill={on ? 'rgba(94,234,212,0.14)' : past ? 'rgba(126,217,167,0.07)' : 'transparent'}
                  stroke={on ? G2 : past ? 'rgba(126,217,167,0.4)' : 'rgba(234,231,224,0.15)'}
                  strokeWidth={1}
                />
                <text x={84 + i * 138} y={432} textAnchor="middle" fontSize={7.5} fill={on ? G2 : past ? OK : FAINT} fontFamily="JetBrains Mono">
                  {i + 1}. {p}
                </text>
              </g>
            )
          })}
        </g>
      </DeckSvg>

      <div className="lagrange-btns">
        <button type="button" className="btn" onClick={replay}>
          ▶ run ceremony
        </button>
        <span className="figtoast">
          {phase === 0 && 'same roster, threshold, unit, amounts — if those differ, the ceremony cannot finish.'}
          {phase === 1 && 'each member evaluates a share for every receiver, seals it, then throws the raw coefficients away.'}
          {phase === 2 && 'hash first, so the last member cannot peek and adapt its polynomial.'}
          {phase === 3 && 'open the G2 commitments; the reveal must match the hash, or it is rejected.'}
          {phase === 4 && 'private evaluations over the authenticated plane; each receiver checks f_j(i)·G2 against the public polynomial.'}
          {phase === 5 && 'sum the verified contributions: k_i = Σ f_j(i). public constant terms sum to K = k·G2. the secret k never exists in one place.'}
          {phase === 6 && 'hash the finished ceremony, collect member signatures — only then activate the keyset.'}
        </span>
      </div>
      <div className="figcap" style={{ marginTop: '0.5rem' }}>
        <span className="dot" />
        <span>
          hash so nobody can adapt, reveal so everyone can check, deliver privately, add. trusted-dealer setup is the
          same end state — but production wants this ceremony so no one machine ever sees <span className="math">k</span>.
        </span>
      </div>
    </div>
  )
}