# Examples

| Example | Needs a mint | What it shows |
|---------|--------------|---------------|
| `wallet_setup.py` | No | Building a wallet on an in-memory and on a file-backed SQLite store, reading balances |
| `token_inspect.py` | No | Decoding a Cashu token and reading its mint, value, memo and proofs |
| `transaction_history.py` | No | Listing transactions and filtering by direction |
| `mint_and_send.py` | Yes | The full lifecycle: mint quote, mint, send, receive back |

The offline examples run against the installed package with no setup:

```bash
python examples/wallet_setup.py
```

`mint_and_send.py` needs a reachable mint:

```bash
CDK_PYTHON_TEST_MINT_URL=https://testnut.cashudevkit.org \
    python examples/mint_and_send.py
```

testnut settles mint quotes on its own, so the example waits rather than asking
you to pay the invoice. Override the wait with
`CDK_PYTHON_MINT_SETTLEMENT_DELAY_SECONDS`.

From the cdk monorepo, `just examples-python` runs the three offline examples as
a smoke check.
