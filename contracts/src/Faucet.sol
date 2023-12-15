// SPDX-License-Identifier: GPL-3.0
pragma solidity ^0.8.13;

contract Faucet {
  address payable public owner;

  uint public withdrawAmount = 1000000000000000;
  uint public lockTime = 1 minutes;

  mapping(address => uint) nextRequestAt;

  event Deposit(address indexed from, uint indexed amount);
  event Transfer(address indexed to, uint indexed amount);
  event Withdraw(address indexed to, uint indexed amount);

  constructor() {
    owner = payable(msg.sender);
  }

  modifier onlyOwner() {
    require(msg.sender == owner, "Only the owner can call this function");
    _;
  }

  function requestFundsByList(address[] memory addresses) external {
    require(addresses.length > 0, "Address list is empty.");

    for (uint i = 0; i < addresses.length; i++) {
      requestFundsByAddress(addresses[i]);
    }
  }

  function requestFundsByAddress(address account) internal {
    require(account != address(0), "Request must not be from zero address");
    require(address(this).balance >= withdrawAmount, "Faucet out of funds");
    require(block.timestamp >= nextRequestAt[account], "Insufficient time between requests");

    nextRequestAt[account] = block.timestamp + lockTime;

    payable(account).transfer(withdrawAmount);
    emit Transfer(account, withdrawAmount);
  }

  function withdrawFunds() external onlyOwner {
    payable(owner).transfer(address(this).balance);
    emit Withdraw(owner, address(this).balance);
  }

  function getNextRequestAt(address account) external view returns (uint) {
    return nextRequestAt[account];
  }

  function setOwner(address payable account) external onlyOwner {
    owner = account;
  }

  function setWithdrawAmount(uint amount) external onlyOwner {
    withdrawAmount = amount;
  }

  function setLockTime(uint amount) external onlyOwner {
    lockTime = amount * 1 minutes;
  }

  receive() external payable {
    emit Deposit(msg.sender, msg.value);
  }
}