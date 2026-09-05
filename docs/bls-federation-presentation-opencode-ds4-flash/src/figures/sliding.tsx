import { useEffect, useState } from 'react'
import { DeckSvg, G1, ROSE, INK, DIM, FAINT, OK } from './graph.tsx'
import { useOnceInView } from '../primitives.tsx'

const MEMBERS = ['M1', 'M2', 'M3', 'M4', 'M5']
const OUTPUTS = ['A', 'B', 'C']
const ROW_Y = [78, 116, 154, 192, 230]
const COL_X = [250, 460, 670]

/* which member signed which output, at each animation step */
function signedAt(step: number, mi: number, oi: number): boolean {
  if (step < 1) return false
  const m = MEMBERS[mi]
  const out = OUTPUTS[oi]
  const p1 = {
    M1: ['A', 'B'],
    M2: ['A', 'B'],
    M3: ['A', 'B'],
    M4: ['B', 'C'],
    M5: ['B', 'C'],
  }[m]!
  const p2 = { M3: 'C', M4: 'C', M5: 'C' } as Record<string, string>
  return p1.includes(out) || (step >= 2 && p2[m] === out)
}

function countAt(step: number, oi: number): number {
  return MEMBERS.reduce((acc, _, mi) => acc + (signedAt(step, mi, oi) ? 1 : 0), 0)
}

export function Sliding() {
  const { ref, inView } = useOnceInView<HTMLDivElement>()
  const [step, setStep] = useState(0)
  const [playId, setPlayId] = useState(0)

  useEffect(() => {
    if (!inView || playId === 0) return
    const t1 = setTimeout(() => setStep(1), 60)
    const t2 = setTimeout(() => setStep(2), 1750)
    return () => {
      clearTimeout(t1)
      clearTimeout(t2)
    }
  }, [playId, inView])

  const replay = () => {
    setStep(0)
    setPlayId((x) => x + 1)
  }

  return (
    <div ref={ref} className="figure">
      <DeckSvg width={960} height={310}>
        {/* output columns */}
        {OUTPUTS.map((o, oi) => (
          <text key={o} x={COL_X[oi]} y={52} textAnchor="middle" fontSize={13} fontWeight={700} fill={INK} fontFamily="JetBrains Mono">
            {o}
          </text>
        ))}
        <text x={620} y={52} textAnchor="middle" fontSize={8.5} fill={FAINT} fontFamily="JetBrains Mono">
          paid quote: 2 outputs
        </text>

        {/* rows + dots */}
        {MEMBERS.map((m, mi) => (
          <g key={m}>
            <text x={140} y={ROW_Y[mi] + 4} textAnchor="middle" fontSize={9.5} fontWeight={700} fill={DIM} fontFamily="JetBrains Mono">
              {m}
            </text>
            {OUTPUTS.map((_, oi) => {
              const on = signedAt(step, mi, oi)
              const isExtra = oi === 2 && on && countAt(step, 2) >= 3
              return (
                <circle
                  key={oi}
                  className="sdot"
                  cx={COL_X[oi]}
                  cy={ROW_Y[mi]}
                  r={8}
                  fill={on ? (isExtra ? ROSE : G1) : 'transparent'}
                  stroke={on ? (isExtra ? ROSE : G1) : 'rgba(234,231,224,0.12)'}
                  strokeWidth={1.3}
                />
              )
            })}
          </g>
        ))}

        {/* per-output tally */}
        <g>
          {OUTPUTS.map((o, oi) => {
            const c = countAt(step, oi)
            const enough = c >= 3
            const extra = oi === 2 && enough
            return (
              <text
                key={o}
                x={COL_X[oi]}
                y={268}
                textAnchor="middle"
                fontSize={9.5}
                fontWeight={700}
                fill={extra ? ROSE : enough ? OK : DIM}
                fontFamily="JetBrains Mono"
              >
                {c === 0 ? '—' : `${c}/3`} {enough ? '✓' : ''}
              </text>
            )
          })}
        </g>

        {/* annotation lines */}
        <text x={150} y={300} textAnchor="start" fontSize={8} fill={FAINT} fontFamily="JetBrains Mono">
          pass 1: M1–M3 sign {`{A,B}`}, M4–M5 sign {`{B,C}`} · pass 2: M3–M5 sign {`{B,C}`} again
        </text>
      </DeckSvg>

      <div className="lagrange-btns">
        <button type="button" className="btn" onClick={replay}>
          ▶ replay attack
        </button>
        <span className="figtoast">
          {step === 0 && 'each member only ever sees a balanced-looking request.'}
          {step === 1 && 'two locally-fine windows. C has only 2 shares…'}
          {step === 2 && (
            <b style={{ color: 'var(--rose)' }}>
              now every column has 3 shares — A, B <em>and</em> C are spendable. paid 2, minted 3.
            </b>
          )}
        </span>
      </div>
      <div className="figcap" style={{ marginTop: '0.5rem' }}>
        <span className="dot" />
        <span>
          threshold-signing mix-and-match. a BLS share is independently valid, so rotating which members see which
          window collects a threshold for <em>every</em> output. the defense is consensus: sign only the one operation
          that won ordering.
        </span>
      </div>
    </div>
  )
}