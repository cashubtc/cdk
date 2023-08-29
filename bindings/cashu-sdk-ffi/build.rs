fn main() {
    uniffi::generate_scaffolding("./src/cashu_sdk.udl").expect("Building the UDL file failed");
}
