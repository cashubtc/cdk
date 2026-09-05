import { useState } from 'react'
import { DeckSvg, G1, INK, DIM, FAINT, OK, FlowParticles } from './graph.tsx'
import { useOnceInView } from '../primitives.tsx'

/* f(x) = 7 + 3x + 2x²  →  k = f(0) = 7  */
const f = (x: number) => 7 + 3 * x + 2 * x * x
const SHARES = [1, 2, 3, 4, 5].map((i) => ({ id: i, k: f(i) }))
const T = 3

/* exact rational: {n, d} */
type Rat = { n: number; d: number }
const gcd = (a: number, b: number): number => (b === 0 ? a : gcd(b, a % b))
function addR(a: Rat, b: Rat): Rat {
  const n = a.n * b.d + b.n * a.d
  const d = a.d * b.d
  const g = gcd(Math.abs(n), Math.abs(d))
  return { n: n / g, d: d / g }
}
function mulR(a: Rat, b: Rat): Rat {
  const n = a.n * b.n
  const d = a.d * b.d
  const g = gcd(Math.abs(n), Math.abs(d))
  return { n: n / g, d: d / g }
}
function divR(a: Rat, b: Rat): Rat {
  return mulR(a, { n: b.d, d: b.n })
}
function fmtR(r: Rat): string {
  if (r.n === 0) return '0'
  let n = r.n
  let d = r.d
  if (d < 0) {
    n = -n
    d = -d
  }
  if (n % d === 0) return `${n / d}`
  return `${n}/${d}`
}

const X0 = 70
const X1 = 900
const Y0 = 20
const Y1 = 252
const mapX = (i: number) => X0 + (i / 5) * (X1 - X0)
const mapY = (v: number) => Y1 - (v / 85) * (Y1 - Y0)

function curve(): string {
  let d = ''
  for (let x = 0; x <= 5.001; x += 0.04) {
    const px = mapX(x)
    const py = mapY(f(x))
    d += (x === 0 ? 'M' : 'L') + ` ${px.toFixed(1)} ${py.toFixed(1)}`
  }
  return d
}

export function Lagrange() {
  const { ref, inView } = useOnceInView<HTMLDivElement>()
  const [sel, setSel] = useState<number[]>([])
  const done = sel.length === T

  const toggle = (id: number) => {
    setSel((s) => {
      if (s.includes(id)) return s.filter((x) => x !== id)
      if (s.length >= T) return s
      return [...s, id]
    })
  }

  const lambdas = done
    ? sel.map((i) => {
        let acc: Rat = { n: 1, d: 1 }
        for (const j of sel) {
          if (j === i) continue
          const num = { n: -j, d: 1 }
          const den = { n: i - j, d: 1 }
          acc = mulR(acc, divR(num, den))
        }
        return { id: i, lam: acc }
      })
    : []

  const sumRat: Rat = lambdas.reduce(
    (acc, { id, lam }) => addR(acc, mulR(lam, { n: f(id), d: 1 })),
    { n: 0, d: 1 },
  )

  return (
    <div ref={ref} className="lagrange">
      <DeckSvg width={960} height={340}>
        {/* axes */}
        <line x1={X0} y1={Y1 + 18} x2={X1} y2={Y1 + 18} stroke="rgba(234,231,224,0.2)" strokeWidth={1.2} />
        <line x1={X0} y1={Y0} x2={X0} y2={Y1 + 18} stroke="rgba(234,231,224,0.2)" strokeWidth={1.2} />
        {/* the polynomial */}
        <path d={curve()} fill="none" stroke="rgba(94,234,212,0.5)" strokeWidth={1.8} strokeDasharray="6 5" />
        <text x={mapX(4.6)} y={mapY(f(4.6)) - 12} textAnchor="middle" fontSize={8.5} fill={FAINT} fontFamily="JetBrains Mono">
          f(x) = 7 + 3x + 2x²
        </text>

        {/* share points + secret point */}
        <g>
          <circle cx={mapX(0)} cy={mapY(7)} r={5} fill={OK} />
          <text x={mapX(0) + 10} y={mapY(7) + 4} fontSize={9.5} fontWeight={700} fill={OK} fontFamily="JetBrains Mono">
            k = f(0) = 7
          </text>
        </g>
        {SHARES.map((s) => {
          const isOn = sel.includes(s.id)
          return (
            <g key={s.id} onClick={() => toggle(s.id)} style={{ cursor: 'pointer' }}>
              <circle cx={mapX(s.id)} cy={mapY(s.k)} r={isOn ? 8 : 7} fill={isOn ? G1 : '#141822'} stroke={isOn ? G1 : 'rgba(234,231,224,0.3)'} strokeWidth={2} />
              <text x={mapX(s.id)} y={mapY(s.k) - 13} textAnchor="middle" fontSize={9} fontWeight={700} fill={isOn ? G1 : INK} fontFamily="JetBrains Mono">
                M{s.id}
              </text>
              <text x={mapX(s.id)} y={mapY(s.k) + 22} textAnchor="middle" fontSize={8.5} fill={isOn ? G1 : DIM} fontFamily="JetBrains Mono">
                k{s.id} = {s.k}
              </text>
            </g>
          )
        })}

        {/* x labels */}
        {SHARES.map((s) => (
          <text key={s.id} x={mapX(s.id)} y={Y1 + 30} textAnchor="middle" fontSize={8} fill={FAINT} fontFamily="JetBrains Mono">
            x = {s.id}
          </text>
        ))}
        <text x={mapX(0)} y={Y1 + 30} textAnchor="middle" fontSize={8} fill={FAINT} fontFamily="JetBrains Mono">
          x = 0
        </text>

        {/* converging particles from selected nodes to the secret */}
        {inView &&
          done &&
          sel.map((id, i) => (
            <FlowParticles
              key={`${id}-${sel.join('')}`}
              path={`M ${mapX(id)} ${mapY(f(id))} L ${mapX(0)} ${mapY(7)}`}
              n={2}
              color={G1}
              size={2.6}
              dur={1}
              begin={i * 0.14}
            />
          ))}
      </DeckSvg>

      <div className="lagrange-ui">
        <div className="lagrange-btns">
          {SHARES.map((s) => {
            const isOn = sel.includes(s.id)
            return (
              <button
                key={s.id}
                type="button"
                className={`nodebtn ${isOn ? 'on-g1' : ''}${done && !isOn ? '' : ''}`}
                onClick={() => toggle(s.id)}
                disabled={done && !isOn}
              >
                M{s.id}
              </button>
            )
          })}
          <button type="button" className="btn" onClick={() => setSel([])}>
            reset
          </button>
        </div>
        <div className="lagrange-result">
          {!done && (
            <span className="figtoast">
              pick any {T} members — click the nodes on the curve or the buttons. every valid triple interpolates to
              the same secret.
            </span>
          )}
          {done && (
            <span className="figtoast">
              S = {'{'}
              {sel.join(', ')}
              {'}'} →{' '}
              {lambdas.map((l, i) => (
                <span key={l.id}>
                  λ<sub>{l.id}</sub> = {fmtR(l.lam)}
                  {i < lambdas.length - 1 ? ', ' : ''}
                </span>
              ))}{' '}
              → <b style={{ color: 'var(--ok)' }}>Σ λᵢ·kᵢ = {fmtR(sumRat)} = k</b>
            </span>
          )}
        </div>
      </div>
      <div className="figcap">
        <span className="dot" />
        <span>
          wallet holds t shares <span className="math">C_i = k_i · B</span>; Lagrange coefficients at zero collapse
          them to <span className="math">C = k · B</span>. any subset of size t gives the same <span className="math">C</span> —
          order does not matter, fewer than t learn nothing about <span className="math">k</span>.
        </span>
      </div>
    </div>
  )
}