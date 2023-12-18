# Asynchronous Bitcoin Worker

## The Issue
There are occasions when the Botanix node requires access to Bitcoin blocks for transaction validation. Making direct calls to a Bitcoin block source can be expensive and lead to synchronization challenges. For instance, during processes like pegin, the reth executor needs to confirm the inclusion of a Bitcoin transaction in a block at a specific height `h`. To achieve this, we can either make network requests at that moment or maintain an accessible list of the most current Bitcoin header in our database. The latter is more favorable and we'll explain why.

## Resolution

To mitigate the need for network requests with each Bitcoin transaction inclusion request, the reth node can manage a the n'th deep Bitcoin block header. During the initial boot-up, the reth node will verify its block header against the current tip and then proceed to download the n'th deep header. All operations crucial for consensus will pause until the node has an opportunity to synchronize.

During IBD block validation nodes need the block header used to validate pegins. Therefore a new Botanix header field will be introduced called `bitcoin_header`.
Consensus will need to check that a block proposer is proposing a valid block header that is n blocks deep.

Additionally, if a block is produced using a block height that is less than the (tip - n), consensus must invalidate this block.
Lastly, if a block is produced using a block height that is greater than (tip - n). That means the current node is behind and must sync before trying to validate again.

### Asynchronous Worker

Upon booting up, the reth node will initiate a new background thread known as the `async_bitcoin_worker`. This worker's responsibilities include maintaining the headers datastructure and performing the initial synchronization.