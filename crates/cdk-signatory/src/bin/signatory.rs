fn main() {
    #[cfg(target_arch = "wasm32")]
    println!("Not supported in wasm32");
    #[cfg(not(target_arch = "wasm32"))]
    {
        use tokio::runtime::Runtime;
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            cdk_signatory::cli::main().await.unwrap();
        });
    }
}
