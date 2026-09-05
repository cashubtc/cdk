import { motion, useInView, useReducedMotion } from 'motion/react'
import { useRef, type ReactNode } from 'react'

/* ---------- shared easing tokens (mirror styles.css) ---------- */

export const EASE_OUT = [0.23, 1, 0.32, 1] as const
export const EASE_IN_OUT = [0.77, 0, 0.175, 1] as const

/* ---------- scroll reveal ---------- */

export function Reveal({
  children,
  delay = 0,
  y = 26,
  className,
  style,
  once = true,
}: {
  children: ReactNode
  delay?: number
  y?: number
  className?: string
  style?: React.CSSProperties
  once?: boolean
}) {
  const reduce = useReducedMotion()
  return (
    <motion.div
      className={className}
      style={style}
      initial={reduce ? { opacity: 0 } : { opacity: 0, y }}
      whileInView={reduce ? { opacity: 1 } : { opacity: 1, y: 0 }}
      viewport={{ once, margin: '-40px' }}
      transition={{ duration: 0.55, delay, ease: EASE_OUT }}
    >
      {children}
    </motion.div>
  )
}

/** Stagger a group of items with short per-item delays. */
export function Stagger({
  children,
  start = 0,
  step = 0.06,
  y = 20,
  className,
}: {
  children: ReactNode
  start?: number
  step?: number
  y?: number
  className?: string
}) {
  return (
    <div className={className}>
      {Array.isArray(children)
        ? children.map((c, i) => (
            <Reveal key={i} delay={start + i * step} y={y}>
              {c}
            </Reveal>
          ))
        : children}
    </div>
  )
}

/* ---------- page scaffolding ---------- */

export function PageHead({
  idx,
  section,
  title,
  lead,
  accent,
  right,
}: {
  idx: string
  section: string
  title: ReactNode
  lead?: ReactNode
  accent?: ReactNode
  right?: ReactNode
}) {
  return (
    <header>
      <Reveal>
        <div className="eyebrow">
          <span className="idx">{idx}</span>
          <span className="tick" />
          <span>{section}</span>
          {right}
        </div>
      </Reveal>
      <Reveal delay={0.05}>
        <h2 className="title">
          {title} {accent}
        </h2>
      </Reveal>
      {lead && (
        <Reveal delay={0.1}>
          <p className="lead">{lead}</p>
        </Reveal>
      )}
    </header>
  )
}

export function Page({
  id,
  children,
  cover,
}: {
  id: string
  children: ReactNode
  cover?: boolean
}) {
  return (
    <section id={id} className={`page${cover ? ' page-cover' : ''}`}>
      <div className={cover ? 'page-cover-inner' : 'page-inner'}>{children}</div>
    </section>
  )
}

/* ---------- math ---------- */

export function M({
  c,
  sub,
  sup,
  children,
  className,
}: {
  c?: string
  sub?: ReactNode
  sup?: ReactNode
  children?: ReactNode
  className?: string
}) {
  return (
    <span className={`${c ?? ''} ${className ?? ''}`.trim()}>
      {children}
      {sub !== undefined && <sub>{sub}</sub>}
      {sup !== undefined && <sup>{sup}</sup>}
    </span>
  )
}

export function Eq({
  children,
  panel,
  className,
}: {
  children: ReactNode
  panel?: boolean
  className?: string
}) {
  return (
    <div className={`eq ${panel ? 'box' : ''} ${className ?? ''}`.trim()}>{children}</div>
  )
}

/* ---------- misc components ---------- */

export function Chip({
  c,
  dot,
  children,
}: {
  c?: string
  dot?: string
  children: ReactNode
}) {
  return (
    <span className="chip">
      {dot && <span className={`dot ${dot}`} />}
      <span className={c}>{children}</span>
    </span>
  )
}

export function Tag({ c, children }: { c?: string; children: ReactNode }) {
  return <span className={`tag ${c ?? ''}`}>{children}</span>
}

export function KV({ pairs }: { pairs: [ReactNode, ReactNode][] }) {
  return (
    <dl className="kv">
      {pairs.map(([k, v], i) => (
        <div key={i}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  )
}

export function Callout({
  tone,
  k,
  children,
}: {
  tone?: 'rose' | 'secp'
  k?: ReactNode
  children: ReactNode
}) {
  return (
    <div className={`callout ${tone ?? ''}`}>
      {k && <span className="k">{k}</span>}
      <span>{children}</span>
    </div>
  )
}

export function Quote({ children, src }: { children: ReactNode; src?: string }) {
  return (
    <blockquote className="quote">
      {children}
      {src && <span className="src">{src}</span>}
    </blockquote>
  )
}

export function Step({ n, children }: { n: number; children: ReactNode }) {
  return (
    <div className="step">
      <span className="n">{n}</span>
      <p>{children}</p>
    </div>
  )
}

export function Steps({ children }: { children: ReactNode }) {
  return <div className="steps">{children}</div>
}

export function Legend({ items }: { items: [string, string][] }) {
  return (
    <div className="legend">
      {items.map(([dot, label], i) => (
        <span className="item" key={i}>
          <span className={`dot ${dot}`} />
          <span>{label}</span>
        </span>
      ))}
    </div>
  )
}

export function FigCap({ children }: { children: ReactNode }) {
  return (
    <div className="figcap">
      <span className="dot" />
      <span>{children}</span>
    </div>
  )
}

/* ---------- in-view gating for figures ---------- */

export function useOnceInView<T extends Element>(margin = '-60px') {
  const ref = useRef<T>(null)
  const inView = useInView(ref, { once: true, margin: margin as never })
  return { ref, inView }
}