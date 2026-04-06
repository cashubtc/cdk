//! Print the full CDK wallet database schema SQL to stdout.
//!
//! This is useful for bootstrapping a new Supabase project or for the CI
//! pipeline that spins up a local Supabase stack and needs to apply the
//! schema before running the integration tests:
//!
//! ```bash
//! cargo run -p cdk-supabase --example print_schema | psql <connection-string>
//! ```

fn main() {
    print!("{}", cdk_supabase::SupabaseWalletDatabase::get_schema_sql());
}
