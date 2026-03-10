/// Connect Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Default)]
pub struct ConnectInfo {
    pub pubkey: String,
    pub address: String,
    pub port: u16,
}

/// Balance response
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default)]
pub struct Balance {
    pub on_chain_spendable: u64,
    pub on_chain_total: u64,
    pub ln: u64,
}
