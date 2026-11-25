# CDK FROST

Application-neutral secp256k1 FROST root-key and participant key-manager
primitives for CDK federations.

The crate deliberately does not depend on BDK, Bitcoin networks, Cashu mint
business logic, federation consensus, or transport. Application adapters must
authorize typed requests before the key manager can consume a nonce and create
a signature share.

Every production key manager must be constructed with one participant-wide
`DurableNonceStore`. The store is a compare-and-swap boundary shared by all
registered applications and must commit before returning success. Secret nonce
material is sealed at rest, removed before FROST round two, and never restored
after a restart or ambiguous outcome. `MemoryNonceStore` exists only for tests
and embedding prototypes; it is not durable production storage.

## Never restore the nonce store from a backup

The redb nonce store is kept in its own file, apart from the mint's SQL
database, on purpose. A FROST nonce used to sign two different messages
reveals the participant's key share. A restored copy of the nonce store would
mark nonces that were already consumed as unused again, so restoring it from a
backup can leak the key share. Keeping it out of the SQL database means a mint
database backup or restore can never bring consumed nonces back.

Operators must therefore exclude the nonce store from backups and never restore
it. If the file is lost or suspect, stop the member, delete the file and start
again: the member generates fresh nonces. A signing session that referenced a
deleted nonce cannot complete with this member and must be started again.
