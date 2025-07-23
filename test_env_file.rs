use std::env;

fn main() {
    // Set up environment variables for testing
    env::set_var("CDK_ITESTS_DIR", "./test_temp_dir");
    
    // Import our function
    use cdk_integration_tests::init_regtest::set_test_urls_with_env_file;
    
    // Call the function with test values
    set_test_urls_with_env_file("127.0.0.1", 8085, 8087);
    
    println!("Test completed successfully!");
}
