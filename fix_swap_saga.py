with open("crates/cdk/src/wallet/swap/saga/mod.rs", "r") as f:
    content = f.read()

content = content.replace(
"""        include_fees: bool,
<<<<<<< HEAD
        proof_reservation: ProofReservation,
=======
        skip_invariant: bool,
>>>>>>> 588bdcdd (feat(wallet): implement NUT-XX Efficient Wallet Recovery)
    ) -> Result<SwapSaga<'a, Prepared>, Error> {""",
"""        include_fees: bool,
        proof_reservation: ProofReservation,
        skip_invariant: bool,
    ) -> Result<SwapSaga<'a, Prepared>, Error> {""")

content = content.replace(
"""                use_p2bk,
                &fee_breakdown,
<<<<<<< HEAD
                proof_reservation,
=======
                skip_invariant,
>>>>>>> 588bdcdd (feat(wallet): implement NUT-XX Efficient Wallet Recovery)
            )""",
"""                use_p2bk,
                &fee_breakdown,
                proof_reservation,
                skip_invariant,
            )""")

with open("crates/cdk/src/wallet/swap/saga/mod.rs", "w") as f:
    f.write(content)
