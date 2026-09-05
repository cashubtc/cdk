import { type ReactNode } from 'react'

/* ============================================================
   SVG graph toolkit — nodes, edges, flowing particles
   all animation runs in SVG (SMIL/CSS), off the JS thread
   ============================================================ */

export type XY = { x: number; y: number }

export function ring(cx: number, cy: number, r: number, n: number, phase = -Math.PI / 2): XY[] {
  return Array.from({ length: n }, (_, i) => ({
    x: cx + r * Math.cos(phase + (2 * Math.PI * i) / n),
    y: cy + r * Math.sin(phase + (2 * Math.PI * i) / n),
  }))
}

export function lerp(a: XY, b: XY, t: number): XY {
  return { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t }
}

export function mid(a: XY, b: XY): XY {
  return lerp(a, b, 0.5)
}

/** Shared marker for G1/G2/secp colors used inside figures. */
export const G1 = '#a78bfa'
export const G2 = '#5eead4'
export const SECP = '#f5b45f'
export const ROSE = '#ff5f6c'
export const OK = '#7ed9a7'
export const INK = '#eae7e0'
export const DIM = '#8f8a9e'
export const FAINT = '#5c5866'
export const PANEL = '#141822'

export type NodeState = 'idle' | 'g1' | 'g2' | 'secp' | 'rose' | 'ok' | 'dim'

const nodeFill: Record<NodeState, string> = {
  idle: '#141822',
  g1: G1,
  g2: G2,
  secp: SECP,
  rose: ROSE,
  ok: OK,
  dim: '#232834',
}

const nodeStroke: Record<NodeState, string> = {
  idle: 'rgba(234,231,224,0.28)',
  g1: G1,
  g2: G2,
  secp: SECP,
  rose: ROSE,
  ok: OK,
  dim: 'rgba(234,231,224,0.14)',
}

const nodeText: Record<NodeState, string> = {
  idle: INK,
  g1: INK,
  g2: '#0a0c11',
  secp: '#0a0c11',
  rose: '#0a0c11',
  ok: '#0a0c11',
  dim: DIM,
}

export function SvgNode({
  p,
  r = 13,
  label,
  state = 'idle',
  sub,
  pulse,
  onClick,
  className,
}: {
  p: XY
  r?: number
  label?: string
  state?: NodeState
  sub?: string
  pulse?: boolean
  onClick?: () => void
  className?: string
}) {
  return (
    <g className={className} onClick={onClick} style={onClick ? { cursor: 'pointer' } : undefined}>
      {pulse && (
        <circle className="motion-svg" cx={p.x} cy={p.y} r={r} fill="none" stroke={nodeStroke[state]} strokeWidth={1.4}>
          <animate attributeName="r" values={`${r};${r + 5};${r}`} dur="2.2s" repeatCount="indefinite" />
          <animate attributeName="opacity" values="0.7;0;0.7" dur="2.2s" repeatCount="indefinite" />
        </circle>
      )}
      <circle cx={p.x} cy={p.y} r={r} fill={nodeFill[state]} stroke={nodeStroke[state]} strokeWidth={state === 'idle' ? 1.4 : 2} />
      {label !== undefined && (
        <text
          x={p.x}
          y={p.y}
          textAnchor="middle"
          dominantBaseline="central"
          fontSize={sub ? 9 : 10.5}
          fontWeight={sub ? 400 : 700}
          fill={nodeText[state]}
        >
          {label}
        </text>
      )}
    </g>
  )
}

/** A straight edge with an optional draw-in reveal and optional label. */
export function SvgEdge({
  a,
  b,
  color = 'rgba(234,231,224,0.22)',
  width = 1.6,
  dash,
  dashed = false,
  label,
  animated = true,
  className,
}: {
  a: XY
  b: XY
  color?: string
  width?: number
  dash?: string
  dashed?: boolean
  label?: string
  animated?: boolean
  className?: string
}) {
  const d = `M ${a.x} ${a.y} L ${b.x} ${b.y}`
  const dashPattern = dashed ? (dash ?? '5 4') : undefined
  return (
    <g className={className}>
      <path
        d={d}
        fill="none"
        stroke={color}
        strokeWidth={width}
        strokeDasharray={dashPattern}
        className="motion-dash"
        pathLength={1000}
        style={animated ? { strokeDashoffset: 1000 } : undefined}
      />
      {label && (
        <text x={mid(a, b).x} y={mid(a, b).y - 6} textAnchor="middle" fontSize={8.5} fill={DIM} fontFamily="JetBrains Mono">
          {label}
        </text>
      )}
    </g>
  )
}

export function FlowParticles({
  path,
  n = 3,
  color = G1,
  size = 2.6,
  dur = 1.15,
  begin = 0,
  className,
}: {
  path: string
  n?: number
  color?: string
  size?: number
  dur?: number
  begin?: number
  className?: string
}) {
  return (
    <g className={`motion-svg ${className ?? ''}`}>
      {Array.from({ length: n }, (_, i) => (
        <circle key={i} r={size} fill={color}>
          <animateMotion
            dur={`${dur}s`}
            begin={`${begin + i * (dur / n)}s`}
            repeatCount="indefinite"
            path={path}
            pathLength={1000}
          />
        </circle>
      ))}
    </g>
  )
}

/** One-shot burst of particles travelling along a path (used after a replay). */
export function OneShotParticles({
  path,
  n = 4,
  color = G1,
  size = 2.6,
  dur = 1.4,
}: {
  path: string
  n?: number
  color?: string
  size?: number
  dur?: number
}) {
  return (
    <g className="motion-svg">
      {Array.from({ length: n }, (_, i) => (
        <circle key={i} r={size} fill={color}>
          <animateMotion
            dur={`${dur}s`}
            begin={`${(i * dur) / (n + 1)}s`}
            repeatCount="1"
            fill="freeze"
            path={path}
            pathLength={1000}
          />
        </circle>
      ))}
    </g>
  )
}

/** A rounded envelope (consensus box / operation card). */
export function EnvelopeBox({
  cx,
  cy,
  w,
  h,
  label,
  color = 'rgba(234,231,224,0.28)',
  fill = '#141822',
  sub,
}: {
  cx: number
  cy: number
  w: number
  h: number
  label: string
  color?: string
  fill?: string
  sub?: string
}) {
  return (
    <g>
      <rect x={cx - w / 2} y={cy - h / 2} width={w} height={h} rx={9} fill={fill} stroke={color} strokeWidth={1.4} />
      <text x={cx} y={cy - 1} textAnchor="middle" fontSize={9.5} fontWeight={700} fill={INK} fontFamily="JetBrains Mono">
        {label}
      </text>
      {sub && (
        <text x={cx} y={cy + 11} textAnchor="middle" fontSize={8} fill={DIM} fontFamily="JetBrains Mono">
          {sub}
        </text>
      )}
    </g>
  )
}

export function FrameLabel({ p, label, color = DIM, fontSize = 9.5 }: { p: XY; label: string; color?: string; fontSize?: number }) {
  return (
    <text x={p.x} y={p.y} textAnchor="middle" fontSize={fontSize} fontWeight={600} fill={color} fontFamily="JetBrains Mono">
      {label}
    </text>
  )
}

/* ============================================================
   legend swatch used inside figures
   ============================================================ */

export function Swatch({ x, y, color, label }: { x: number; y: number; color: string; label: string }) {
  return (
    <g>
      <circle cx={x} cy={y} r={4} fill={color} />
      <text x={x + 7} y={y + 3.5} fontSize={8} fill={DIM} fontFamily="JetBrains Mono">
        {label}
      </text>
    </g>
  )
}

/* ============================================================
   deck-level helpers
   ============================================================ */

export function DeckSvg({
  width,
  height,
  children,
  className,
  role = 'img',
}: {
  width: number
  height: number
  children: ReactNode
  className?: string
  role?: string
}) {
  return (
    <svg
      className={className}
      viewBox={`0 0 ${width} ${height}`}
      role={role}
      aria-label="animated federation diagram"
      width="100%"
      style={{ height: 'auto' }}
    >
      {children}
    </svg>
  )
}

export function pathOf(points: XY[]): string {
  return points.map((p, i) => `${i === 0 ? 'M' : 'L'} ${p.x} ${p.y}`).join(' ')
}