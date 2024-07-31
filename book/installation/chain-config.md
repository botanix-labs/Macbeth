
# Setting up federation.toml
Federation.toml defines your federation. It includes federation public keys and socket addresses.
You may also define initial account states. The one account that should not change is `0x0Ea320990B44236A0cEd0ecC0Fd2b2df33071e78` this is the botanix minting contract that mints Bitcoin on a valid pegin.

An example of a two person federation would be 
```
botanix-fee-recipient="0xb8c03cb8C9bAC79c53926E3C66344C13452105f5"

[[federation-member-public-key]]
key="039bef292b80427d355cecb89eda8a50a7d2196a93d73dade5a0c4a07cd334815d"
socket-addr="127.0.0.1:30303"

[[federation-member-public-key]]
key="02bdc272b244f717604fffe659d2d98205d1e6764fdf453d1631f42c2db4d8d710"
socket-addr="127.0.0.1:30304"
```

### What is the additional fee-recipient
The Botanix federation requires you to setup an additional fee recipient. Any 20-byte eth address will work for this field.
The additional fee recepient is traditionally the party responsible for setting up the federation, coordinating setup and maintaince and responding to emergencies.
For their additional responsibilities they will recieve 20% of all block fees. 

### How to generate your federation.toml
TBD
