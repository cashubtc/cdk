with open("crates/cdk/src/wallet/receive/saga/mod.rs", "r") as f:
    content = f.read()

content = content.replace(
"""                &fee_breakdown,
<<<<<<< HEAD
                ProofReservation::Skip,
=======
                false,
>>>>>>> 588bdcdd (feat(wallet): implement NUT-XX Efficient Wallet Recovery)
            )""",
"""                &fee_breakdown,
                ProofReservation::Skip,
                false,
            )""")

with open("crates/cdk/src/wallet/receive/saga/mod.rs", "w") as f:
    f.write(content)
