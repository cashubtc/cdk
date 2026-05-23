# NUT-31: Payjoin for Onchain Payment Method

`optional`

`depends on: NUT-04 NUT-05 NUT-20 NUT-30`

This draft extends the NUT-30 `onchain` payment method with structured
Payjoin fields. The existing `request` field remains a Bitcoin address and is
kept as the fallback destination for wallets and mints that do not support
Payjoin.

This draft specifies BIP77 Payjoin v2 only. Future versions may define
additional parameter objects under the same wrapper.

## Types

### `PayjoinV2`

```json
{
  "endpoint": "https://payjoin.example/pj",
  "ohttp_relay": "https://relay.example",
  "ohttp_keys": "encoded-ohttp-keys",
  "receiver_key": "encoded-receiver-session-key",
  "expires_at": 1701704757,
  "required": false
}
```

- `endpoint`: string, BIP77 mailbox endpoint equivalent to the `pj` value.
- `ohttp_relay`: string, relay URL used for OHTTP requests.
- `ohttp_keys`: string, encoded OHTTP key material needed by the sender.
- `receiver_key`: string, encoded receiver session key.
- `expires_at`: nullable Unix timestamp after which the Payjoin parameters
  should not be used.
- `required`: boolean. If true, direct fallback payment to `request` is not
  allowed.

### Payjoin Wrapper

```json
{
  "version": 2,
  "params": {
    "endpoint": "https://payjoin.example/pj",
    "ohttp_relay": "https://relay.example",
    "ohttp_keys": "encoded-ohttp-keys",
    "receiver_key": "encoded-receiver-session-key",
    "expires_at": 1701704757,
    "required": false
  }
}
```

Implementations MUST NOT require wallets to pass a BIP21 or BIP321 URI. If an
implementation library internally requires a URI, it is assembled from
`request`, `amount` when present, and `payjoin`.

## Mint Quote Request

`PostMintQuoteOnchainRequest` MAY include:

```json
{
  "unit": "sat",
  "pubkey": "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
  "payjoin": {
    "version": 2,
    "required": false
  }
}
```

This asks the mint to return Payjoin-capable deposit instructions. If
`payjoin` is absent, quote creation behaves exactly as NUT-30.

## Mint Quote Response

`PostMintQuoteOnchainResponse` MAY include:

```json
{
  "quote": "DSGLX9kevM...",
  "request": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
  "unit": "sat",
  "expiry": 1701704757,
  "pubkey": "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
  "amount_paid": 0,
  "amount_issued": 0,
  "payjoin": {
    "version": 2,
    "params": {
      "endpoint": "https://payjoin.example/pj",
      "ohttp_relay": "https://relay.example",
      "ohttp_keys": "encoded-ohttp-keys",
      "receiver_key": "encoded-receiver-session-key",
      "expires_at": 1701704757,
      "required": false
    }
  }
}
```

`request` remains the fallback Bitcoin address. If `payjoin.params.required` is
true, wallets that cannot complete Payjoin MUST NOT pay `request` directly.

## Melt Quote Request

`PostMeltQuoteOnchainRequest` MAY include:

```json
{
  "request": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
  "unit": "sat",
  "amount": 100000,
  "payjoin": {
    "version": 2,
    "params": {
      "endpoint": "https://payjoin.example/pj",
      "ohttp_relay": "https://relay.example",
      "ohttp_keys": "encoded-ohttp-keys",
      "receiver_key": "encoded-receiver-session-key",
      "expires_at": 1701704757,
      "required": false
    }
  }
}
```

`request` remains the fallback destination address. `amount` remains the amount
to send.

## Melt Quote Response

`PostMeltQuoteOnchainResponse` MAY include:

```json
{
  "quote": "TRmjduhIsPxd...",
  "amount": 100000,
  "unit": "sat",
  "state": "UNPAID",
  "expiry": 1701704757,
  "request": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
  "fee_options": [
    {
      "fee_index": 0,
      "fee_reserve": 5000,
      "estimated_blocks": 1
    }
  ],
  "selected_fee_index": null,
  "outpoint": null,
  "payjoin": {
    "version": 2,
    "required": false
  }
}
```

The presence of `payjoin` confirms that the mint accepted Payjoin v2 parameters
for this quote.

## Fallback And Version Handling

If `payjoin` is absent, behavior is exactly NUT-30.

Unknown Payjoin versions MUST be ignored unless marked `required`. Required
Payjoin with an unsupported version MUST reject quote creation.

When `required` is true and Payjoin cannot be completed, the sender MUST fail
without broadcasting a fallback transaction to `request`. When `required` is
false and Payjoin cannot be completed, the sender MAY fall back to the direct
onchain payment described by NUT-30.
