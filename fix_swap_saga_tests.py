with open("crates/cdk/src/wallet/swap/saga/mod.rs", "r") as f:
    content = f.read()

content = content.replace(
"""                false,
                false,
                ProofReservation::Reserve,
            )""",
"""                false,
                false,
                ProofReservation::Reserve,
                false,
            )""")

content = content.replace(
"""                false,
                false,
                ProofReservation::Skip,
            )""",
"""                false,
                false,
                ProofReservation::Skip,
                false,
            )""")

with open("crates/cdk/src/wallet/swap/saga/mod.rs", "w") as f:
    f.write(content)
