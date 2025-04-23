use ethers::contract::Abigen;

fn main() {
    // generate minting contract abi
    Abigen::new("MintContract", "mint_contract_abi.json")
        .expect("Error reading mint contract json abi")
        .generate()
        .expect("Error generating mint contract rust definitions")
        .write_to_file("./src/mint_contract_abi.rs")
        .expect("Error writing mint contract rust file");

    // generate minting attack contract abi
    Abigen::new("MintAttackContract", "mint_attack_contract_abi.json")
        .expect("Error reading mint attack contract json abi")
        .generate()
        .expect("Error generating mint attack contract rust definitions")
        .write_to_file("./src/mint_attack_contract_abi.rs")
        .expect("Error writing mint attack contract rust file");

    // generate multi mint helper contract abi
    Abigen::new(
        "MultiMintHelperContract",
        "multi_mint_helper_abi.json",
    )
    .expect("Error reading multi mint helper contract json abi")
    .generate()
    .expect("Error generating multi mint helper contract rust definitions")
    .write_to_file("./src/multi_mint_helper_abi.rs")
    .expect("Error writing multi mint helper contract rust file");
}
