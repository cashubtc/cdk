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
