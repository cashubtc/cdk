with open("crates/cdk/src/wallet/melt/saga/mod.rs", "r") as f:
    content = f.read()

content = content.replace(
"""                ProofInfo::new_with_operations(
                    p,
                    self.wallet.mint_url.clone(),
                    State::Reserved,
                    self.wallet.unit.clone(),
                    Some(operation_id),
                    None,
                )""",
"""                ProofInfo::new_with_operations(
                    p,
                    self.wallet.mint_url.clone(),
                    State::Reserved,
                    self.wallet.unit.clone(),
                    Some(operation_id),
                    None,
                    None,
                )""")

content = content.replace(
"""                ProofInfo::new_with_operations(
                    p,
                    self.wallet.mint_url.clone(),
                    State::Pending,
                    self.wallet.unit.clone(),
                    Some(operation_id),
                    None,
                )""",
"""                ProofInfo::new_with_operations(
                    p,
                    self.wallet.mint_url.clone(),
                    State::Pending,
                    self.wallet.unit.clone(),
                    Some(operation_id),
                    None,
                    None,
                )""")

with open("crates/cdk/src/wallet/melt/saga/mod.rs", "w") as f:
    f.write(content)
