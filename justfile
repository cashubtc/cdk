precommit:
	cargo check --no-default-features --features mint
	cargo check --no-default-features --features wallet
	cargo check --no-default-features --features blocking
	typos
	cargo test