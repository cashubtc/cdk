# NUT-31: Payjoin for Onchain Payment Method

`optional`

`depends on: NUT-04 NUT-05 NUT-20 NUT-30`

This draft extends the NUT-30 `onchain` payment method with structured
Payjoin fields. The existing `request` field remains a Bitcoin address and is
kept as the fallback destination for wallets and mints that do not support
Payjoin.

This draft specifies BIP77 Payjoin v2 only.

## Types

### `PayjoinV2`

```json
{
  "endpoint": "https://payjoin.example/pj",
  "ohttp_keys": "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ",
  "receiver_key": "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG",
  "expires_at": 1701704757
}
```

- `endpoint`: string, BIP77 mailbox endpoint URL without the receiver fragment
  parameters.
- `ohttp_keys`: string, BIP77-encoded OHTTP key material needed by the sender,
  without the `OH1` prefix. It decodes to one key identifier byte followed by a
  33-byte compressed secp256k1 public key.
- `receiver_key`: string, BIP77-encoded receiver session key, without the
  `RK1` prefix. It decodes to a 33-byte compressed secp256k1 public key.
- `expires_at`: Unix timestamp after which the Payjoin parameters should not
  be used.

The OHTTP relay is intentionally not part of this structure. Per BIP77, the
sender wallet chooses the relay it will use.

Implementations MUST NOT require wallets to pass a BIP21 or BIP321 URI. If an
implementation library internally requires a URI, it is assembled from
`request`, `amount` when present, and `payjoin`.

## Mint Quote Request

`PostMintQuoteOnchainRequest` is unchanged from NUT-30:

```json
{
  "unit": "sat",
  "pubkey": "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac"
}
```

If a mint is configured for Payjoin receive support, it MAY automatically
include Payjoin-capable deposit instructions in the response. Wallets do not
negotiate Payjoin support in mint quote requests.

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
    "endpoint": "https://payjoin.example/pj",
    "ohttp_keys": "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ",
    "receiver_key": "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG",
    "expires_at": 1701704757
  }
}
```

`request` remains the fallback Bitcoin address. Wallets MAY attempt Payjoin, but
MAY also pay `request` directly.

## Melt Quote Request

`PostMeltQuoteOnchainRequest` MAY include:

```json
{
  "request": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
  "unit": "sat",
  "amount": 100000,
  "payjoin": {
    "endpoint": "https://payjoin.example/pj",
    "ohttp_keys": "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ",
    "receiver_key": "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG",
    "expires_at": 1701704757
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
    "endpoint": "https://payjoin.example/pj",
    "ohttp_keys": "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ",
    "receiver_key": "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG",
    "expires_at": 1701704757
  }
}
```

The presence of `payjoin` confirms that the mint accepted Payjoin v2 parameters
for this quote.

## Fallback Handling

If `payjoin` is absent from any response, behavior is exactly NUT-30.

If Payjoin cannot be completed, the sender MAY fall back to the direct onchain
payment described by NUT-30.
