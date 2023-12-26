# Staking

# Helpful reading and reference

[Upgrading Ethereum by Ben Edgington](https://eth2book.info/capella/)

# Purpose

Staking of BTC by authority members helps secure the network through concensus. Honest authority members will receive the block subsidy reward. Malicious authority members will be fully slashed resulting in a total loss of staked bitcoin and removal from the list of authority members.

# Assumptions and Context

- The genesis block header will include one authority member in the extra data header field which will produce the first (n) post genesis blocks. The extra data header field is the source of truth when determining authority members.
- A known staking contract will exist on Botanix which holds a whitelist of potential stakers and a list of stakers. Only authority members are included in the whitelist and can be stakers. Authority members move from the whitelist to the stakers list once they have staked the required funds. The contract will be deployed with the genesis block authority member as the only member in the stakers list with all other authority members in the whitelist. The initial staker is Botanix.
  - After the genesis block, the initial staker will add an authority member to the list in the extra data header field once a member in the whitelist has staked the required funds and moved to the stakers list.
  - Newly added members may add additional members to the extra data header field in the same manner as the initital genesis block staker.
  - The process of initial whitelisted members moving to the stakers list and being included in the extra data header field will conclude once all initial whitelisted members have become stakers.
  - A staker is not allowed to withdraw funds and leave the federation.
  - A staker may be voted out by a majority of authority members because of a slashable offense. The offender's staked funds will be transferred to the member who identified the slashable offense.
    - This is the whistleblower reward.
- During an epoch, authority members can vote to add or remove an authority member. A member can only be voted out because of a slashable offense.
  - If a new member is voted in, they will be added to the whitelist and will need to stake the required funds to move to the stakers list and be included in the extra data header field.
  - If an existing member is voted out, they will be removed from the whitelist and stakers list and the extra data header field.
- An authority member must initiate a vote to remove an authority member if they identify the authority member has committed a slashable offense and a vote has not been started.
- An authority member is required to vote to remove a member that has committed a slashable offense. The offense must be confirmed as an actual slashable offense before casting a vote.

# Staking contract requirements

// TODO: update function signatures to include parameters

- The contract will be upgradeable
- Total staked amount: `uint256 public totalStaked`
- Whitelist of authority members: `address[] public whitelist`
- List of stakers: `address[] public stakers;`
- Mapping of stakers and their balances: `mapping(address => uint256) public stakerBalances`
- `addMember()` that adds a member to the `whitelist`
- `removeMember()` that removes a member from both the `whitelist` and `stakers` list
- `stake()` method that updates fields accordingly:
  - update `stakers` list after confirming the sender is in the `whitelist`
  - update `totalStaked`
  - update `stakerBalances` list
- `slash()` that transfers all staked funds of a member who committed a slashable offense to the whistleblower:
  - must confirm the offending member is not in the `whitelist` and `stakers` list.
  - update the `stakerBalances` list

# Block Proposal

An authority member is included in the extra data header field once they have moved from the staking contract's `whitelist` to the `stakers` list. This happens once the member has staked the required funds. Only members in the extra data header field can propose a block.

# Authority member responsibilities

// TODO: add more responsibilities

- A member is expected to propose a valid block when in-turn.
- A member is expected to participate in bitcoin signing rounds.
- A member is expected to start the removal process of an authority member due to a slashable offense.
- A member is expected to cast a vote to remove an authority member due to a slashable offense.

# Voting

Voting to remove or add an authority member occurs across the length of an epoch:

- Voting may occur across two epochs:
  - For example:
    - an epoch is 100 blocks
    - the epoch starting block was at block 500 and concludes at block 600
    - the vote started at block 520
    - the vote will conclude at block 620 in the next epoch
- A vote is finalized in the final block of an epoch if the vote has concluded within the current epoch.

# Rewards

- There is no base reward since Botanix is pegged 1:1 to Bitcoin
- A member that identifies a slashable offense receives all the offender's funds once the offender has been voted out.

# Penalties

There are no penalities only fully slashable events

# Slashing

- Offenses
  - Proposing multiple blocks while in turn
  - Proposing a block with an invalid pegin or pegout
  - Not being live for bitcoin transaction signing rounds
