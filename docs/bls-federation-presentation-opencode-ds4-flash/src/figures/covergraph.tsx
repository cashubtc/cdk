import { DeckSvg, SvgNode, SvgEdge, FlowParticles, ring, G2, G1, FAINT, mid, type XY } from './graph.tsx'
import { useOnceInView } from '../primitives.tsx'

const LABELS = ['M1', 'M2', 'M3', 'M4', 'M5']

export function CoverGraph() {
  const { ref, inView } = useOnceInView<HTMLDivElement>('-40px')
  const members = ring(760, 215, 128, 5, -Math.PI / 2)
  const wallet: XY = { x: 210, y: 215 }

  return (
    <div ref={ref}>
      <DeckSvg width={960} height={430}>
        {/* private mesh */}
        {members.map((m, i) => {
          const next = members[(i + 1) % members.length]
          return (
            <SvgEdge key={`mesh${i}`} a={m} b={next} color="rgba(234,231,224,0.16)" dashed width={1.3} animated={inView} />
          )
        })}
        {/* public fan-out */}
        {members.map((m, i) => (
          <SvgEdge key={`star${i}`} a={wallet} b={m} color="rgba(167,139,250,0.42)" width={1.8} animated={inView} />
        ))}
        {inView &&
          members.map((m, i) => (
            <FlowParticles
              key={`p${i}`}
              path={`M ${wallet.x} ${wallet.y} L ${m.x} ${m.y}`}
              n={2}
              color={i === 3 ? G2 : G1}
              size={2.6}
              dur={1.15}
              begin={i * 0.16}
            />
          ))}
        {inView && (
          <FlowParticles
            path={members
              .map((m, i) => {
                const nxt = members[(i + 1) % members.length]
                return `M ${m.x} ${m.y} L ${nxt.x} ${nxt.y}`
              })
              .join(' ')}
            n={4}
            color={FAINT}
            size={1.7}
            dur={5}
            begin={0.4}
          />
        )}

        {/* nodes */}
        <SvgNode p={wallet} r={15} label="W" state="idle" pulse />
        {members.map((m, i) => (
          <SvgNode
            key={`n${i}`}
            p={m}
            label={LABELS[i]}
            state={i === 3 ? 'dim' : 'idle'}
            pulse={i === 3}
          />
        ))}

        {/* plane labels */}
        <text x={mid(wallet, members[1]).x - 26} y={mid(wallet, members[1]).y - 16} fontSize={9.5} fill={G1} fontFamily="JetBrains Mono" fontWeight={600}>
          public · fan-out
        </text>
        <text x={mid(members[0], members[1]).x - 30} y={mid(members[0], members[1]).y - 16} fontSize={9.5} fill={FAINT} fontFamily="JetBrains Mono" fontWeight={600}>
          private · consensus
        </text>
      </DeckSvg>
      <div className="figcap">
        <span className="dot" />
        <span>
          the whole federation on one screen — wallet fans the same Cashu request to every member (public), members
          agree on the exact operation before signing (private).{' '}
          <span className="cf">M4 is offline; the wallet needs only a threshold of shares.</span>
        </span>
      </div>
    </div>
  )
}