use ethers::contract::Abigen;

fn main() {
    // generate contract abi
    Abigen::new("MintContract", "mint_contract_abi.json")
        .expect("Error reading mint contract json abi")
        .generate()
        .expect("Error generating mint contract rust definitions")
        .write_to_file("./src/minting.rs")
        .expect("Error writing mint contract rust file");
}
