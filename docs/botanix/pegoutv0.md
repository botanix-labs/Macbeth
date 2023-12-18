# Pegout Version 0

In simple terms, the consensus code is responsible for parsing the emissions from the 'burn' topic. The 'burn' operation should emit both a Bitcoin amount in satoshis and a Bitcoin address. It's important to note that neither of these values undergo validation within the smart contract; instead, the validation process takes place within the consensus code.

Once validation is triggered, the 'reth' node needs to make a call to the 'BTC_Server' to sign for the pegout transaction. Subsequently, it should broadcast this transaction to the Bitcoin network.

The logic for validating pegout operations will be contained within the 'botanix_lib' crate. On the other hand, the consensus logic for handling pegout attempts should reside in the 'revm/executor.rs' file. If a pegout attempt is found to be invalid, it's essential to set the EIP-658 receipt's success flag to 'false'. Otherwise a miner will include the transaction in the next block,
