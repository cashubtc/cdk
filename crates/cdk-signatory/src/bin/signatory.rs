#[cfg(not(target_arch = "wasm32"))]
mod cli;

fn main() {
    #[cfg(target_arch = "wasm32")]
    println!("Not supported in wasm32");
    #[cfg(not(target_arch = "wasm32"))]
    {
        use tokio::runtime::Runtime;
        let rt = Runtime::new().expect("Runtime created");
        rt.block_on(async {
            cli::cli_main().await.expect("cli error");
        });
    }
}
