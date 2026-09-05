import { useEffect, useState } from 'react'
import { useOnceInView } from '../primitives.tsx'

export type LedgerStep = {
  label: string
  count?: string
  tone?: 'g2' | 'g1' | 'secp' | 'rose' | 'ok'
  note?: string
}

const toneColor: Record<NonNullable<LedgerStep['tone']>, string> = {
  g2: 'var(--g2)',
  g1: 'var(--g1)',
  secp: 'var(--secp)',
  rose: 'var(--rose)',
  ok: 'var(--ok)',
}

/** A horizontal strip of consensus operations that lights up one at a time. */
export function Ledger({ steps, flow }: { steps: LedgerStep[]; flow: string }) {
  const { ref, inView } = useOnceInView<HTMLDivElement>()
  const [n, setN] = useState(0)
  const [playId, setPlayId] = useState(0)

  useEffect(() => {
    if (!inView || playId === 0) return
    const timers: ReturnType<typeof setTimeout>[] = []
    steps.forEach((_, i) => timers.push(setTimeout(() => setN(i + 1), 250 + i * 330)))
    return () => timers.forEach(clearTimeout)
  }, [playId, inView, steps])

  const replay = () => {
    setN(0)
    setPlayId((x) => x + 1)
  }

  return (
    <div ref={ref} className="ledger">
      <div className="ledger-strip">
        {steps.map((s, i) => {
          const on = i < n
          const color = s.tone ? toneColor[s.tone] : 'var(--line-2)'
          return (
            <div key={i} className={`ledger-chip${on ? ' on' : ''}`} style={on ? { borderColor: color } : undefined}>
              <span className="ledger-count" style={on ? { color } : undefined}>
                {s.count ?? i + 1}
              </span>
              <span className="ledger-label">{s.label}</span>
              {on && s.note && <span className="ledger-note">{s.note}</span>}
            </div>
          )
        })}
        <span className="ledger-arrow">→</span>
        <span className="ledger-out">{flow}</span>
      </div>
      <div className="lagrange-btns">
        <button type="button" className="btn" onClick={replay}>
          ▶ replay flow
        </button>
        <span className="figtoast">
          counts are distinct accepted operations — duplicates of the same <span className="math">operation_id</span>{' '}
          do not add another fact. off-log after accept: BLS shares, FROST rounds.
        </span>
      </div>
    </div>
  )
}