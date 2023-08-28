fn main() {
    uniffi::generate_scaffolding("./src/cashu.udl").expect("Building the UDL file failed");
}
