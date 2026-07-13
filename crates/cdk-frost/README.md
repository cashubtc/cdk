# CDK FROST

Application-neutral secp256k1 FROST root-key and participant key-manager
primitives for CDK federations.

The crate deliberately does not depend on BDK, Bitcoin networks, Cashu mint
business logic, federation consensus, or transport. Application adapters must
authorize typed requests before the key manager can consume a nonce and create
a signature share.
