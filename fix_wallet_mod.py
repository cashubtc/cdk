with open("crates/cdk/src/wallet/mod.rs", "r") as f:
    content = f.read()

content = content.replace(
"""            let saga = Box::pin(saga.prepare(
                None,
                crate::amount::SplitTarget::None,
                proofs_to_swap,
                None,
                false,
                false,
                true,
            ))""",
"""            let saga = Box::pin(saga.prepare(
                None,
                crate::amount::SplitTarget::None,
                proofs_to_swap,
                None,
                false,
                false,
                crate::wallet::swap::ProofReservation::Reserve,
                true,
            ))""")

with open("crates/cdk/src/wallet/mod.rs", "w") as f:
    f.write(content)
