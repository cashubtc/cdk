#!/usr/bin/env python3
"""Decode a Cashu token and print what it carries.

Runs entirely offline. Pass a token as the first argument, or let it fall back
to the NUT-00 test vector.
"""

import sys

import cdk

SAMPLE_TOKEN = (
    "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91IHZlcnkgbXVjaC4ifQ"
)


def main():
    encoded = sys.argv[1] if len(sys.argv) > 1 else SAMPLE_TOKEN

    token = cdk.Token.decode(encoded)

    print(f"mint:   {token.mint_url().url}")
    print(f"value:  {token.value().value} {token.unit()}")
    print(f"memo:   {token.memo()}")

    proofs = token.proofs_simple()
    print(f"proofs: {len(proofs)}")
    for proof in proofs:
        print(f"  {proof.amount.value:>6} sat  keyset {proof.keyset_id}")

    assert cdk.Token.decode(token.encode()).value().value == token.value().value
    print("\nre-encoding round trips")


if __name__ == "__main__":
    main()
