with open("crates/cdk/src/wallet/swap/saga/mod.rs", "r") as f:
    content = f.read()

content = content.replace(
"""                use_p2bk,
                true,
                ProofReservation::Reserve,
            )
            .await?;""",
"""                use_p2bk,
                true,
                ProofReservation::Reserve,
                false,
            )
            .await?;""")

content = content.replace(
"""                use_p2bk,
                true,
                ProofReservation::Skip,
            )
            .await?;""",
"""                use_p2bk,
                true,
                ProofReservation::Skip,
                false,
            )
            .await?;""")

with open("crates/cdk/src/wallet/swap/saga/mod.rs", "w") as f:
    f.write(content)
