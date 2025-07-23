use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Sets test URLs in environment variables and writes them to a .env file
/// in the work/temp_dir. Creates the file if it doesn't exist.
pub fn set_test_urls_with_env_file(mint_addr: &str, cln_port: u16, lnd_port: u16) {
    // Set environment variables as before
    env::set_var(
        "CDK_TEST_MINT_URL",
        format!("http://{}:{}", mint_addr, cln_port),
    );
    env::set_var(
        "CDK_TEST_MINT_URL_2",
        format!("http://{}:{}", mint_addr, lnd_port),
    );

    // Get the temp directory from environment or use a default
    let temp_dir = env::var("CDK_ITESTS_DIR").unwrap_or_else(|_| {
        // Fallback to current directory + temp if not set
        "./temp_dir".to_string()
    });

    // Create the path to the .env file
    let env_file_path = Path::new(&temp_dir).join(".env");
    
    // Create the temp directory if it doesn't exist
    if let Some(parent) = env_file_path.parent() {
        std::fs::create_dir_all(parent).expect("Failed to create temp directory");
    }

    // Write environment variables to .env file
    let mut file = File::create(&env_file_path).expect("Failed to create .env file");
    
    writeln!(
        file,
        "CDK_TEST_MINT_URL=http://{}:{}",
        mint_addr, cln_port
    ).expect("Failed to write CDK_TEST_MINT_URL to .env file");
    
    writeln!(
        file,
        "CDK_TEST_MINT_URL_2=http://{}:{}",
        mint_addr, lnd_port
    ).expect("Failed to write CDK_TEST_MINT_URL_2 to .env file");
    
    println!("Environment variables written to {}", env_file_path.display());
}
