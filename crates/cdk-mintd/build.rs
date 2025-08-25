fn main() {
    // Check that at least one database feature is enabled
    let has_database = cfg!(feature = "sqlite") || cfg!(feature = "postgres");

    if !has_database {
        panic!(
            "cdk-mintd requires at least one database backend to be enabled.\n\
             Available database features: sqlite, postgres\n\
             Example: cargo build --features sqlite"
        );
    }

    // Check that at least one Lightning backend is enabled
    let has_lightning_backend = cfg!(feature = "cln")
        || cfg!(feature = "lnd")
        || cfg!(feature = "lnbits")
        || cfg!(feature = "fakewallet")
        || cfg!(feature = "grpc-processor")
        || cfg!(feature = "ldk-node");

    if !has_lightning_backend {
        panic!(
            "cdk-mintd requires at least one Lightning backend to be enabled.\n\
             Available Lightning backends: cln, lnd, lnbits, fakewallet, grpc-processor\n\
             Example: cargo build --features \"sqlite fakewallet\""
        );
    }

    println!("cargo:rerun-if-changed=build.rs");
}
