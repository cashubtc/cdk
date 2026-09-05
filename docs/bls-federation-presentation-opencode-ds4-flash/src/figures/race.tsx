import { useEffect, useState } from 'react'
import { DeckSvg, G2, ROSE, INK, FAINT, OK, PANEL, FlowParticles, EnvelopeBox, SvgNode, type XY } from './graph.tsx'
import { useOnceInView } from '../primitives.tsx'

const members: XY[] = [
  { x: 700, y: 120 },
  { x: 800, y: 90 },
  { x: 820, y: 170 },
  { x: 800, y: 250 },
  { x: 700, y: 280 },
]

export function Race() {
  const { ref, inView } = useOnceInView<HTMLDivElement>()
  const [step, setStep] = useState(0)
  const [playId, setPlayId] = useState(0)

  useEffect(() => {
    if (!inView || playId === 0) return
    const t1 = setTimeout(() => setStep(1), 120)
    const t2 = setTimeout(() => setStep(2), 1900)
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
      <DeckSvg width={960} height={330}>
        {/* wallet */}
        <SvgNode p={{ x: 70, y: 165 }} label="W" r={15} state="idle" pulse />
        <text x={70} y={196} textAnchor="middle" fontSize={8.5} fill={FAINT} fontFamily="JetBrains Mono">
          wallet
        </text>

        {/* envelopes */}
        <EnvelopeBox cx={250} cy={85} w={200} h={46} label="S_AB = hash(P1,P2 → A,B)" sub="accepted" color={step >= 1 ? G2 : 'rgba(234,231,224,0.2)'} />
        <EnvelopeBox cx={250} cy={245} w={200} h={46} label="S_BC = hash(P1,P2 → B,C)" sub={step >= 1 ? 'rejected — conflict' : ''} color={step >= 1 ? ROSE : 'rgba(234,231,224,0.2)'} />

        {/* consensus box */}
        <g>
          <rect x={420} y={130} width={190} height={70} rx={12} fill={PANEL} stroke={step >= 1 ? G2 : 'rgba(234,231,224,0.25)'} strokeWidth={1.6} />
          <text x={515} y={156} textAnchor="middle" fontSize={10} fontWeight={700} fill={INK} fontFamily="JetBrains Mono">
            consensus box
          </text>
          <text x={515} y={174} textAnchor="middle" fontSize={8} fill={step >= 1 ? G2 : FAINT} fontFamily="JetBrains Mono">
            {step >= 1 ? 'one winner: S_AB' : 'orders the exact envelope'}
          </text>
        </g>

        {/* members */}
        {members.map((m, i) => (
          <g key={i}>
            <SvgNode p={m} label={`M${i + 1}`} r={13} state="idle" />
            {step >= 2 && (
              <>
                <text x={m.x} y={m.y + 26} textAnchor="middle" fontSize={7.5} fill={OK} fontFamily="JetBrains Mono">
                  A,B ✓
                </text>
              </>
            )}
          </g>
        ))}
        {/* C gets no shares */}
        <text x={760} y={318} textAnchor="middle" fontSize={8.5} fill={step >= 2 ? ROSE : FAINT} fontFamily="JetBrains Mono">
          C → not in the accepted op → no shares anywhere
        </text>

        {/* flows */}
        {inView && step >= 1 && (
          <>
            <FlowParticles path="M 70 165 L 250 85" n={3} color={G2} size={2.4} dur={0.9} />
            <FlowParticles path="M 70 165 L 250 245" n={3} color={ROSE} size={2.4} dur={0.9} />
            <FlowParticles path="M 250 85 L 420 165" n={4} color={G2} size={2.4} dur={1} />
            {members.map((m, i) => (
              <FlowParticles key={i} path={`M 515 165 L ${m.x} ${m.y}`} n={1} color={G2} size={2} dur={1.4} begin={i * 0.12} />
            ))}
          </>
        )}
      </DeckSvg>

      <div className="lagrange-btns">
        <button type="button" className="btn" onClick={replay}>
          ▶ replay
        </button>
        <span className="figtoast">
          {step === 0 && 'the wallet fans the same swap to everyone; members wrap it as an envelope and submit.'}
          {step === 1 && 'two different envelopes, two different IDs. the box accepts only one — suppose S_AB wins.'}
          {step === 2 && (
            <b style={{ color: 'var(--ok)' }}>
              every honest member applies the same story: spend P1,P2, sign only A,B. S_BC dies with TokenAlreadySpent.
            </b>
          )}
        </span>
      </div>
      <div className="figcap" style={{ marginTop: '0.5rem' }}>
        <span className="dot" />
        <span>
          same bytes, same ID. change one output, different ID. a member does not sign because it saw an HTTP request —
          it signs because that exact envelope came out of the box as accepted.
        </span>
      </div>
    </div>
  )
}