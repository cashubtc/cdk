# CDK FFI Python tests

The deterministic suite checks the generated Python module, SQLite transaction
and callback behavior, portable wallet construction, multi-mint construction,
and the absence of legacy advanced wallet objects.

```bash
just ffi-test
```

This builds a release library, generates Python bindings, copies the library
next to the module, and runs:

- `test_transactions.py` for database transaction semantics;
- `test_kvstore.py` for key/value storage callbacks;
- `test_rate_limit.py` for offline portable-wallet construction and API shape.

The separate live test uses `https://testnut.cashudevkit.org`:

```bash
just ffi-test-live-python
```

It exercises `MintSession`, typed on-chain payment quoting, `PaymentPlan`, both
payment-confirmation outcomes, pending-operation reconstruction, and online
synchronization.

## Troubleshooting

If `cdk_ffi` cannot be imported, regenerate bindings with
`just ffi-generate python`. If the dynamic library is missing, run
`cargo build --release -p cdk-ffi`. Use `just ffi-api-check` to diagnose an
unexpected portable object-surface change.
