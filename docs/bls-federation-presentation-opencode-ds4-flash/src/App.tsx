import { useCallback, useEffect, useRef, useState } from 'react'
import { motion, useScroll, useReducedMotion } from 'motion/react'
import { DECK } from './deck.tsx'

export default function App() {
  const { scrollYProgress } = useScroll()
  const reduce = useReducedMotion()
  const sectionRefs = useRef<(HTMLElement | null)[]>([])
  const n = DECK.length
  const [active, setActive] = useState(0)

  useEffect(() => {
    const pick = () => {
      const mid = window.innerHeight / 2
      let best = 0
      let bestDist = Infinity
      sectionRefs.current.forEach((el, i) => {
        if (!el) return
        const r = el.getBoundingClientRect()
        const d = Math.abs(r.top + r.height / 2 - mid)
        if (d < bestDist) {
          bestDist = d
          best = i
        }
      })
      setActive(best)
    }
    pick()
    window.addEventListener('scroll', pick, { passive: true })
    window.addEventListener('resize', pick)
    return () => {
      window.removeEventListener('scroll', pick)
      window.removeEventListener('resize', pick)
    }
  }, [])

  const goTo = useCallback(
    (i: number) => {
      const el = sectionRefs.current[i]
      if (el) {
        el.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'start' })
      }
    },
    [reduce],
  )

  const onKey = useCallback(
    (e: KeyboardEvent) => {
      let next: number | null = null
      switch (e.key) {
        case 'ArrowDown':
        case 'PageDown':
        case ' ':
          next = Math.min(n - 1, active + 1)
          break
        case 'ArrowUp':
        case 'PageUp':
          next = Math.max(0, active - 1)
          break
        case 'Home':
          next = 0
          break
        case 'End':
          next = n - 1
          break
        default:
          return
      }
      e.preventDefault()
      goTo(next)
    },
    [goTo, active, n],
  )

  useEffect(() => {
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onKey])

  return (
    <>
      <motion.div className="progressbar" style={{ scaleX: scrollYProgress }} />
      <aside className="rail" aria-label="Deck sections">
        <div className="brand">
          <span className="mark" />
          <span className="name">CASHU · BLS</span>
        </div>
        <nav>
          {DECK.map((p, i) => (
            <button
              key={p.id}
              type="button"
              className={`item${i === active ? ' active' : ''}`}
              onClick={() => goTo(i)}
            >
              <span className="num">{p.num}</span>
              <span className="lbl">{p.label}</span>
            </button>
          ))}
        </nav>
        <div className="keyhint">
          <span className="kbd">↑</span>
          <span className="kbd">↓</span>
          <span className="kbd">spc</span>
          <span className="kbd">home</span>
          <span className="kbd">end</span>
          <br />
          page-by-page, scroll or keys
        </div>
      </aside>
      <div className="counter">
        <span className="now">{String(active + 1).padStart(2, '0')}</span>
        <span>/ {String(n).padStart(2, '0')}</span>
      </div>
      {DECK.map((p, i) => (
        <section
          id={p.id}
          key={p.id}
          className={`page${p.cover ? ' page-cover' : ''}`}
          ref={(el) => void (sectionRefs.current[i] = el)}
        >
          <div className={p.cover ? 'page-cover-inner' : 'page-inner'}>
            <p.render />
          </div>
        </section>
      ))}
    </>
  )
}