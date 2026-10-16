# Recovering late melt change

A paid payment can precede the mint's change signatures. The wallet now keeps a
`change_pending` melt saga when neither the paid response nor restore supplies
change. The payment receipt remains completed and its input proofs remain spent.
Repeated recovery updates the same receipt and its fee once change is available;
it never sends the payment again. Until then, the receipt uses the fee reserve.
An explicit empty change response closes a zero-change claim. An absent response
keeps its derivation data so it can still collect a later refund.

Existing saga rows remain readable without a database migration. The new state
is serialized in the existing saga state field. Older SDKs cannot read this state:
finish pending change collection before downgrading a wallet database. Do not
remove these records to force a downgrade. All SDK bindings need rebuilt native
libraries before applications can ship this behavior; no FFI API signature changed.
Applications pinned to older published native bindings do not get this SDK fix
merely by changing their UI.

This protects newly finalized payments. A prior SDK may already have deleted a
payment's recovery record. Such payments may require seed-based proof restore;
they cannot be reconstructed from a completed receipt alone.

The regression matrix uses separate, checksum-pinned CDK mint 0.17.7 and 0.18.1
releases with fake Lightning backends. It loses the melt response, temporarily
omits paid-quote change, and returns an empty restore response, then reopens the
wallet's SQLite database and verifies the late refund is collected exactly once
without another melt request:

```sh
python3 misc/test-melt-change-versions.py
```

The runner requires Python 3, GitHub CLI, and the repository's Rust toolchain. It
supports macOS arm64 and Linux x86_64. All mints and proxies bind to loopback and
use temporary test stores. The test does not establish real Lightning routing or
liquidity. The CI workflow explicitly runs this matrix; ordinary unit runs leave
the external-process integration test ignored.
