//! Subcommands for the mint RPC CLI

/// Module for fetching the mint's public metadata
mod get_info;
/// Module for rotating to the next keyset
mod rotate_next_keyset;
/// Module for updating mint contact information
mod update_contact;
/// Module for enabling or disabling minting and melting
mod update_disabled;
/// Module for updating the mint's icon URL
mod update_icon_url;
/// Module for updating the mint's long description
mod update_long_description;
/// Module for updating melt (NUT-05) payment method settings
mod update_melt_method;
/// Module for updating mint (NUT-04) payment method settings
mod update_mint_method;
/// Module for updating mint quote state
mod update_mint_quote_state;
/// Module for updating the mint's message of the day
mod update_motd;
/// Module for updating the mint's name
mod update_name;
/// Module for updating the mint's short description
mod update_short_description;
/// Module for updating the mint's terms of service URL
mod update_tos_url;
/// Module for updating quote time-to-live settings
mod update_ttl;
/// Module for managing mint URLs
mod update_urls;
/// Module for inspecting the mint's BDK on-chain wallet.
mod wallet;

pub use get_info::get_info;
pub use rotate_next_keyset::{rotate_next_keyset, RotateNextKeysetCommand};
pub use update_contact::{add_contact, remove_contact, AddContactCommand, RemoveContactCommand};
pub use update_disabled::{update_disabled, UpdateDisabledCommand};
pub use update_icon_url::{update_icon_url, UpdateIconUrlCommand};
pub use update_long_description::{update_long_description, UpdateLongDescriptionCommand};
pub use update_melt_method::{update_melt_method, UpdateMeltMethodCommand};
pub use update_mint_method::{update_mint_method, UpdateMintMethodCommand};
pub use update_mint_quote_state::{update_mint_quote_state, UpdateMintQuoteStateCommand};
pub use update_motd::{update_motd, UpdateMotdCommand};
pub use update_name::{update_name, UpdateNameCommand};
pub use update_short_description::{update_short_description, UpdateShortDescriptionCommand};
pub use update_tos_url::{update_tos_url, UpdateTosUrlCommand};
pub use update_ttl::{get_quote_ttl, update_quote_ttl, UpdateQuoteTtlCommand};
pub use update_urls::{add_url, remove_url, AddUrlCommand, RemoveUrlCommand};
pub use wallet::{
    create_wallet_deposit_address, get_wallet_balance, list_wallet_addresses,
    list_wallet_transactions, WalletPaginationCommand,
};
