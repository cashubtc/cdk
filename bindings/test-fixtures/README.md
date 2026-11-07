# Wallet preparation fixtures

These fixed, artificial inputs support the offline Swift and Kotlin custom-store
regression tests. They have no value and must never be used with a real mint.
Proof/quote JSON uses CDK's canonical representation; keyset metadata uses the
FFI representation accepted by `decodeKeySetInfo`.
