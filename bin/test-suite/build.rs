use ethers::contract::Abigen;

fn main() {
    // generate contract abi
    Abigen::new("MintContract", "mint_contract_abi.json")
        .expect("Error reading mint contract json abi")
        .generate()
        .expect("Error generating mint contract rust defintions")
        .write_to_file("./src/mint_contract_abi.rs")
        .expect("Error writing mint contract rust file");
}
