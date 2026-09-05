import type { ComponentType } from 'react'
import Cover from './pages/cover.tsx'
import Naming from './pages/naming.tsx'
import Layers from './pages/layers.tsx'
import WhyBls from './pages/whybls.tsx'
import Keyset from './pages/keyset.tsx'
import Blocks from './pages/blocks.tsx'
import Derivation from './pages/derivation.tsx'
import LifecyclePage from './pages/lifecycle.tsx'
import Mixed from './pages/mixed.tsx'
import Fork from './pages/fork.tsx'
import Threshold from './pages/threshold.tsx'
import MixMatch from './pages/mixmatch.tsx'
import Consensus from './pages/consensus.tsx'
import TransportPage from './pages/transport.tsx'
import DkgPage from './pages/dkg.tsx'
import SigAll from './pages/sigall.tsx'
import Funding from './pages/funding.tsx'
import Frost from './pages/frost.tsx'
import BdkMint from './pages/bdkmint.tsx'
import BdkMelt from './pages/bdkmelt.tsx'
import Swap from './pages/swap.tsx'
import Wire from './pages/wire.tsx'
import Roster from './pages/roster.tsx'
import Crash from './pages/crash.tsx'
import Out from './pages/out.tsx'

export type DeckPage = {
  id: string
  num: string
  label: string
  cover?: boolean
  render: ComponentType
}

export const DECK: DeckPage[] = [
  { id: 'cover', num: '00', label: 'cover', cover: true, render: Cover },
  { id: 'naming', num: '01', label: 'the naming trap', render: Naming },
  { id: 'layers', num: '02', label: 'two layers, one checkout', render: Layers },
  { id: 'why-bls', num: '03', label: 'why bls first', render: WhyBls },
  { id: 'keyset-v3', num: '04', label: 'keyset v3 switch', render: Keyset },
  { id: 'crypto-blocks', num: '05', label: 'crypto building blocks', render: Blocks },
  { id: 'derivation', num: '06', label: 'deterministic derivation', render: Derivation },
  { id: 'lifecycle', num: '07', label: 'proof lifecycle', render: LifecyclePage },
  { id: 'mixed', num: '08', label: 'mixed-curve safety', render: Mixed },
  { id: 'fork', num: '09', label: 'secp vs bls', render: Fork },
  { id: 'threshold', num: '10', label: 'threshold bls', render: Threshold },
  { id: 'mixmatch', num: '11', label: 'mix-and-match', render: MixMatch },
  { id: 'consensus', num: '12', label: 'consensus first', render: Consensus },
  { id: 'transport', num: '13', label: 'transport · two planes', render: TransportPage },
  { id: 'dkg', num: '14', label: 'bls dkg', render: DkgPage },
  { id: 'sigall', num: '15', label: 'sig_all · open hole', render: SigAll },
  { id: 'funding', num: '16', label: 'funding backends', render: Funding },
  { id: 'frost', num: '17', label: 'frost dkg', render: Frost },
  { id: 'bdk-mint', num: '18', label: 'bdk mint', render: BdkMint },
  { id: 'bdk-melt', num: '19', label: 'bdk melt', render: BdkMelt },
  { id: 'swap', num: '20', label: 'swap step by step', render: Swap },
  { id: 'wire', num: '21', label: 'consensus on the wire', render: Wire },
  { id: 'roster', num: '22', label: 'roster = constitution', render: Roster },
  { id: 'crash', num: '23', label: 'backup & catch-up', render: Crash },
  { id: 'out', num: '24', label: 'open questions', render: Out },
]