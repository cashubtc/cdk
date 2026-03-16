with open("crates/cdk/src/wallet/swap/mod.rs", "r") as f:
    content = f.read()

content = content.replace(
"""                include_fees,
<<<<<<< HEAD
                proof_reservation,
=======
                false,
>>>>>>> 588bdcdd (feat(wallet): implement NUT-XX Efficient Wallet Recovery)
            )""",
"""                include_fees,
                proof_reservation,
                false,
            )""")

content = content.replace(
"""        proofs_fee_breakdown: &ProofsFeeBreakdown,
<<<<<<< HEAD
        proof_reservation: ProofReservation,
=======
        skip_invariant: bool,
>>>>>>> 588bdcdd (feat(wallet): implement NUT-XX Efficient Wallet Recovery)
    ) -> Result<PreSwap, Error> {""",
"""        proofs_fee_breakdown: &ProofsFeeBreakdown,
        proof_reservation: ProofReservation,
        skip_invariant: bool,
    ) -> Result<PreSwap, Error> {""")

with open("crates/cdk/src/wallet/swap/mod.rs", "w") as f:
    f.write(content)
