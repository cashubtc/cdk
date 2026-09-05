# Cashu · BLS Federation — developer deck

An interactive, scroll-driven presentation built from [`docs/FEDERATION_NOTES.md`](../FEDERATION_NOTES.md).
Tells the story `main → bls → bls-federation`: single-signer BLS12-381 keysets first, then threshold issuance,
consensus, DKG, transport, and the funding backends.

## Run

```bash
npm install
npm run dev       # → http://localhost:5173
```

`npm run build` / `npm run preview` for a production build.

## How the deck works

- 25 full-viewport pages, one per section of the notes, stacked with vertical scroll and snap (`scroll-snap-type: proximity`).
- Navigate with the mouse wheel, the chapter rail on the left, the arrow keys, `Space`, `Home`, `End`.
- `prefers-reduced-motion` is respected — movement is dropped, structure and color transitions stay.

## Interactive figures

- **Lagrange interpolation** (`threshold`) — click any 3 of 5 members; exact rational Lagrange coefficients converge to `k = f(0)`.
- **Sliding window attack** (`mixmatch`) — replay the two-pass 3-of-5 window rotation and watch all three outputs reach a threshold.
- **Envelope race** (`consensus`) — `S_AB` vs `S_BC`, one winner, `C` never gets shares.
- **Transport** (`transport`) — the public star (wallet fan-out) and the private mesh, with an HTTP / Iroh toggle.
- **BLS DKG ceremony** (`dkg`) — commit → reveal → private delivery → sum, with the tiny 3-member example from the notes.
- **Ledger strips** (`bdk-mint`, `bdk-melt`, `wire`) — consensus operations light up in order, with happy-path counts.
- **Cover graph** — the two-plane federation on one screen.

## Design language

- Mono-forward "living spec sheet" (`JetBrains Mono`) with `Instrument Serif` italic pull-quotes.
- Algebra is color-coded: G1 violet (blinded messages/signatures), G2 teal (public keys), secp amber (legacy),
  rose (attacks/open holes), green (accepted).
- Motion follows the `animate` / `emil-design-eng` playbook: ease-out entrances under 300 ms, 30–80 ms staggers,
  SVG/SMIL animation off the main thread, `transform`/`opacity` only.

## Layout

```
src/
  App.tsx            deck shell (snap scroll, rail, scrollspy, keyboard nav)
  deck.tsx           page registry (25 sections)
  primitives.tsx     Reveal, PageHead, Eq/M math, chips, steps, useOnceInView
  styles.css         design tokens + layout
  figures/           graph toolkit + animated figures
  pages/             one file per page
```
