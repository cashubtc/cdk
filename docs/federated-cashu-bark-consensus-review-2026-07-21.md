# Federated Cashu Bark consensus message review

Date: 2026-07-21

## Scope and terminology

This review counts logical CDK operations ordered through AlephBFT. It does
not count raw AlephBFT protocol packets, Ark RPCs, mailbox notifications, or
authenticated peer-to-peer FROST messages.

Let:

- `n` be the number of federation members;
- `t` be the Bark/FROST signature threshold; and
- `q` be the configured CDK payment-observation threshold.

The Nocean deployment is normally a five-member federation with `t = 3` and
`q = 3`.

Member observations make the exact number nondeterministic. The transition
requiring an observation quorum can proceed once the threshold is reached,
but concurrent members may already have submitted additional matching
observations. The formulas below therefore distinguish the minimum required
number from the possible five-member range.

## Successful incoming BOLT11 payment

The incoming Bark flow has six single consensus operations:

1. `ReceiveIntent`
2. `ReceivePrepared`
3. `ReceiveHtlcsReady`
4. `ClaimPlanned`
5. `ReceivePreimageDisclosed`
6. `ReceiveClaimed`

It also requires three sets of member attestations:

- at least `t` matching Bark observations that the incoming HTLC VTXOs are
  ready;
- at least `t` matching Bark observations validating the claimed output
  VTXOs; and
- at least `q` ordinary `MintQuotePayment` observations to make the Cashu
  mint quote paid.

The minimum number of consensus entries is therefore:

```text
6 + 2t + q
```

For a five-member federation with `t = 3` and `q = 3`, the minimum is:

```text
6 + 6 + 3 = 15 consensus entries
```

With concurrent observations from up to all five members, the practical
range is approximately 15 to 21 entries:

```text
ReceiveIntent
ReceivePrepared
3-5 x HTLC-ready Bark MemberObservation
ReceiveHtlcsReady
ClaimPlanned
[FROST/virtual-MuSig2 peer exchange outside AlephBFT]
ReceivePreimageDisclosed
ReceiveClaimed
3-5 x claim-result Bark MemberObservation
3-5 x MintQuotePayment observation
```

The wallet's subsequent `POST /mint` and issuance/signature-share work are
separate Cashu operations and are not included in these Bark payment counts.

## Successful outgoing BOLT11 payment

The successful outgoing path has four single consensus operations:

1. `SendPlanned`
2. `SendHtlcsCosigned`
3. `PaymentInitiated`
4. `PaymentSucceeded`

It additionally requires:

- at least `t` Bark success observations validating the result and payment
  preimage; and
- at least `q` ordinary `MeltQuotePayment` observations to finalize the
  Cashu melt.

The minimum number of consensus entries is:

```text
4 + t + q
```

For `t = 3` and `q = 3`:

```text
4 + 3 + 3 = 10 consensus entries
```

For five members, the practical range is approximately 10 to 14 entries:

```text
SendPlanned
[FROST/virtual-MuSig2 peer exchange outside AlephBFT]
SendHtlcsCosigned
PaymentInitiated
PaymentSucceeded
3-5 x Bark success MemberObservation
3-5 x MeltQuotePayment observation
```

## Failed outgoing BOLT11 payment

A terminal outgoing failure requires a safe revocation before the Cashu melt
can fail and release its reserved value:

```text
SendPlanned
SendHtlcsCosigned
PaymentInitiated
t x Bark failure MemberObservation
PaymentFailed
RevocationPlanned
[FROST/virtual-MuSig2 revocation exchange outside AlephBFT]
RevocationCompleted
t x recovered-result Bark MemberObservation
q x MeltQuotePayment failure observation
```

There are six single operations plus three observation groups, so the minimum
is:

```text
6 + 2t + q
```

For `t = 3` and `q = 3`, this is 15 consensus entries. With five members
submitting concurrently, the practical range is approximately 15 to 21.

## Summary

| Bark flow | General minimum | 3-of-5 minimum | Approximate 5-member range |
|---|---:|---:|---:|
| Incoming success | `6 + 2t + q` | 15 | 15-21 |
| Outgoing success | `4 + t + q` | 10 | 10-14 |
| Outgoing failure with revocation | `6 + 2t + q` | 15 | 15-21 |

## Message layers not included in the totals

The totals above should not be interpreted as total network messages or
round trips. A Bark payment also involves other layers:

- AlephBFT exchanges its own proposal, broadcast, acknowledgement, and
  finalization traffic to order each logical CDK operation.
- FROST and virtual MuSig2 exchange nonce commitments and signature shares
  over authenticated member-to-member transport after an exact action plan
  has been accepted.
- Bark talks to the Ark server for invoice creation, status/evidence lookup,
  HTLC preparation, payment initiation, claim, or revocation as applicable.
- Wallet HTTP requests and optional fan-out to federation members are outside
  the consensus-entry count.

Consequently, reducing the number of consensus entries primarily reduces
consensus finalization latency. It does not by itself remove Ark polling,
FROST round trips, or wallet-to-member fan-out.
