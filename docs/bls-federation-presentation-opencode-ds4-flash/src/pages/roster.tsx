import { PageHead, Reveal, Quote } from '../primitives.tsx'

export default function Roster() {
  return (
    <>
      <PageHead
        idx="22"
        section="genesis"
        title={
          <>
            The roster is the <span className="cd">constitution</span>
          </>
        }
        lead={
          <>
            The federation is not “whoever answers at these five IPs today.” It is a frozen public document — every
            consensus envelope carries <span className="math">federation_id</span>, and a member not on that document
            cannot authorize anything.
          </>
        }
      />
      <Reveal delay={0.1}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">what the ID binds</h3>
            <p className="note" style={{ margin: 0 }}>
              <span className="math">FederationId</span> = SHA-256 over setup authorization, member_count,{' '}
              <span className="math">T</span>, <span className="math">C</span>, and — per member, in order —{' '}
              <span className="math">member_id</span>, <span className="math">public_mint_url</span>,{' '}
              <span className="math">federation_api_url</span>, secp256k1 identity public key. change any of those
              strings or keys and you have a <em>new</em> federation: old journal, old DKG transcripts, old FROST
              roots, old wallets do not follow you.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">config_digest</h3>
            <p className="note" style={{ margin: 0 }}>
              a second hash over the <em>entire</em> <span className="math">FederationConfig</span>. wallets check it
              in each member's mint-info advertisement — fields outside the ID (wallet protocol version, payment
              observation policy, checkpoint policy, FROST/on-chain policy, BLS keysets) still must match across the
              roster, or wallets refuse to aggregate. the only consensus mutation of the signing surface: keyset
              rotation.
            </p>
          </div>
        </div>
      </Reveal>
      <Reveal delay={0.16}>
        <Quote>
          If you put a hostname in the constitution, changing the hostname founds a new country.
        </Quote>
      </Reveal>
      <Reveal delay={0.22}>
        <pre className="codeblock">{`mint-a.example.com expires, the operator wants mint-a.net:

- update local nginx      → the other members still dial the old host
- edit one member's config → their derived ID no longer matches
- tell wallets the new URL → mint-info digest no longer matches

no rotation for locators. the honest path is a new setup:
new proposal, new authorization, new ID, new dual DKG, migrate users.

already frozen → new federation: add/remove/replace a member,
rotate an identity key, change T or C, change observation quorum,
change Bitcoin network / depth / fee policy, swap HTTPS ↔ Iroh.`}</pre>
      </Reveal>
      <Reveal delay={0.28}>
        <div className="split-2">
          <div className="col">
            <h3 className="col-title">why config is consensus</h3>
            <p className="note" style={{ margin: 0 }}>
              if the roster lived only in each operator's config.toml, members would disagree about who may submit,
              who may observe payments, and which URL is canonical — wallets would fan out to different people, and a
              Byzantine member could advertise a look-alike roster. so the public config is part of the agreed world;
              startup, DKG, catch-up and wallet import fail closed on mismatch. they do not “repair” a URL.
            </p>
          </div>
          <div className="col">
            <h3 className="col-h">still open</h3>
            <ul className="col-list">
              <li>locator vs identity — hash only member ID + key, treat URLs as rotatable advertisement</li>
              <li>what belongs in genesis vs policy (observation quorum, fee caps, depth)</li>
              <li>member repair vs federation replacement (catch-up restores state, not identity)</li>
              <li>user-visible migration story when operators must recreate</li>
            </ul>
          </div>
        </div>
      </Reveal>
    </>
  )
}