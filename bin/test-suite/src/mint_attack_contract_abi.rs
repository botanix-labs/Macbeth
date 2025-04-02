pub use mint_attack_contract::*;
/// This module was auto-generated with ethers-rs Abigen.
/// More information at: <https://github.com/gakonst/ethers-rs>
#[allow(
    clippy::enum_variant_names,
    clippy::too_many_arguments,
    clippy::upper_case_acronyms,
    clippy::type_complexity,
    dead_code,
    non_camel_case_types,
)]
pub mod mint_attack_contract {
    #[allow(deprecated)]
    fn __abi() -> ::ethers::core::abi::Abi {
        ::ethers::core::abi::ethabi::Contract {
            constructor: ::core::option::Option::None,
            functions: ::core::convert::From::from([
                (
                    ::std::borrow::ToOwned::to_owned("mintingContract"),
                    ::std::vec![
                        ::ethers::core::abi::ethabi::Function {
                            name: ::std::borrow::ToOwned::to_owned("mintingContract"),
                            inputs: ::std::vec![],
                            outputs: ::std::vec![
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::string::String::new(),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Address,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("contract IMintable"),
                                    ),
                                },
                            ],
                            constant: ::core::option::Option::None,
                            state_mutability: ::ethers::core::abi::ethabi::StateMutability::View,
                        },
                    ],
                ),
                (
                    ::std::borrow::ToOwned::to_owned("passThroughBurn"),
                    ::std::vec![
                        ::ethers::core::abi::ethabi::Function {
                            name: ::std::borrow::ToOwned::to_owned("passThroughBurn"),
                            inputs: ::std::vec![
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("destination"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Bytes,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("bytes"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("data"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Bytes,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("bytes"),
                                    ),
                                },
                            ],
                            outputs: ::std::vec![
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("success"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Bool,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("bool"),
                                    ),
                                },
                            ],
                            constant: ::core::option::Option::None,
                            state_mutability: ::ethers::core::abi::ethabi::StateMutability::Payable,
                        },
                    ],
                ),
                (
                    ::std::borrow::ToOwned::to_owned("passThroughMint"),
                    ::std::vec![
                        ::ethers::core::abi::ethabi::Function {
                            name: ::std::borrow::ToOwned::to_owned("passThroughMint"),
                            inputs: ::std::vec![
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("destination"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Address,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("address"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("amount"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Uint(
                                        256usize,
                                    ),
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("uint256"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned(
                                        "bitcoinBlockHeight",
                                    ),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Uint(32usize),
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("uint32"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("metadata"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Bytes,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("bytes"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("refundAddress"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Address,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("address"),
                                    ),
                                },
                            ],
                            outputs: ::std::vec![],
                            constant: ::core::option::Option::None,
                            state_mutability: ::ethers::core::abi::ethabi::StateMutability::NonPayable,
                        },
                    ],
                ),
            ]),
            events: ::std::collections::BTreeMap::new(),
            errors: ::std::collections::BTreeMap::new(),
            receive: false,
            fallback: false,
        }
    }
    ///The parsed JSON ABI of the contract.
    pub static MINTATTACKCONTRACT_ABI: ::ethers::contract::Lazy<
        ::ethers::core::abi::Abi,
    > = ::ethers::contract::Lazy::new(__abi);
    #[rustfmt::skip]
    const __BYTECODE: &[u8] = b"`\x80`@R4\x80\x15a\0\x10W`\0\x80\xFD[Pa\x070\x80a\0 `\09`\0\xF3\xFE`\x80`@R`\x046\x10a\x004W`\x005`\xE0\x1C\x80c\x03V\x80\x12\x14a\09W\x80c\xCB\xAC\x7F\xF6\x14a\0bW\x80c\xD2\xF6\xF6}\x14a\0\x92W[`\0\x80\xFD[4\x80\x15a\0EW`\0\x80\xFD[Pa\0``\x04\x806\x03\x81\x01\x90a\0[\x91\x90a\x03TV[a\0\xBDV[\0[a\0|`\x04\x806\x03\x81\x01\x90a\0w\x91\x90a\x03\xEEV[a\x01NV[`@Qa\0\x89\x91\x90a\x04\x8AV[`@Q\x80\x91\x03\x90\xF3[4\x80\x15a\0\x9EW`\0\x80\xFD[Pa\0\xA7a\x01\xFDV[`@Qa\0\xB4\x91\x90a\x05\x04V[`@Q\x80\x91\x03\x90\xF3[s\x0E\xA3 \x99\x0BD#j\x0C\xED\x0E\xCC\x0F\xD2\xB2\xDF3\x07\x1Exs\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16c_\xE0?E\x87\x87\x87\x87\x87\x87`@Q\x87c\xFF\xFF\xFF\xFF\x16`\xE0\x1B\x81R`\x04\x01a\x01\x14\x96\x95\x94\x93\x92\x91\x90a\x05\xAAV[`\0`@Q\x80\x83\x03\x81`\0\x87\x80;\x15\x80\x15a\x01.W`\0\x80\xFD[PZ\xF1\x15\x80\x15a\x01BW=`\0\x80>=`\0\xFD[PPPPPPPPPPV[`\0s\x0E\xA3 \x99\x0BD#j\x0C\xED\x0E\xCC\x0F\xD2\xB2\xDF3\x07\x1Exs\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16c\xA5\xD0\xBB\x93`\x024a\x01\x8D\x91\x90a\x065V[\x87\x87\x87\x87`@Q\x86c\xFF\xFF\xFF\xFF\x16`\xE0\x1B\x81R`\x04\x01a\x01\xB0\x94\x93\x92\x91\x90a\x06fV[` `@Q\x80\x83\x03\x81\x85\x88Z\xF1\x15\x80\x15a\x01\xCEW=`\0\x80>=`\0\xFD[PPPPP`@Q=`\x1F\x19`\x1F\x82\x01\x16\x82\x01\x80`@RP\x81\x01\x90a\x01\xF3\x91\x90a\x06\xCDV[\x90P\x94\x93PPPPV[s\x0E\xA3 \x99\x0BD#j\x0C\xED\x0E\xCC\x0F\xD2\xB2\xDF3\x07\x1Ex\x81V[`\0\x80\xFD[`\0\x80\xFD[`\0s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[`\0a\x02J\x82a\x02\x1FV[\x90P\x91\x90PV[a\x02Z\x81a\x02?V[\x81\x14a\x02eW`\0\x80\xFD[PV[`\0\x815\x90Pa\x02w\x81a\x02QV[\x92\x91PPV[`\0\x81\x90P\x91\x90PV[a\x02\x90\x81a\x02}V[\x81\x14a\x02\x9BW`\0\x80\xFD[PV[`\0\x815\x90Pa\x02\xAD\x81a\x02\x87V[\x92\x91PPV[`\0c\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[a\x02\xCC\x81a\x02\xB3V[\x81\x14a\x02\xD7W`\0\x80\xFD[PV[`\0\x815\x90Pa\x02\xE9\x81a\x02\xC3V[\x92\x91PPV[`\0\x80\xFD[`\0\x80\xFD[`\0\x80\xFD[`\0\x80\x83`\x1F\x84\x01\x12a\x03\x14Wa\x03\x13a\x02\xEFV[[\x825\x90Pg\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x031Wa\x030a\x02\xF4V[[` \x83\x01\x91P\x83`\x01\x82\x02\x83\x01\x11\x15a\x03MWa\x03La\x02\xF9V[[\x92P\x92\x90PV[`\0\x80`\0\x80`\0\x80`\xA0\x87\x89\x03\x12\x15a\x03qWa\x03pa\x02\x15V[[`\0a\x03\x7F\x89\x82\x8A\x01a\x02hV[\x96PP` a\x03\x90\x89\x82\x8A\x01a\x02\x9EV[\x95PP`@a\x03\xA1\x89\x82\x8A\x01a\x02\xDAV[\x94PP``\x87\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x03\xC2Wa\x03\xC1a\x02\x1AV[[a\x03\xCE\x89\x82\x8A\x01a\x02\xFEV[\x93P\x93PP`\x80a\x03\xE1\x89\x82\x8A\x01a\x02hV[\x91PP\x92\x95P\x92\x95P\x92\x95V[`\0\x80`\0\x80`@\x85\x87\x03\x12\x15a\x04\x08Wa\x04\x07a\x02\x15V[[`\0\x85\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x04&Wa\x04%a\x02\x1AV[[a\x042\x87\x82\x88\x01a\x02\xFEV[\x94P\x94PP` \x85\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x04UWa\x04Ta\x02\x1AV[[a\x04a\x87\x82\x88\x01a\x02\xFEV[\x92P\x92PP\x92\x95\x91\x94P\x92PV[`\0\x81\x15\x15\x90P\x91\x90PV[a\x04\x84\x81a\x04oV[\x82RPPV[`\0` \x82\x01\x90Pa\x04\x9F`\0\x83\x01\x84a\x04{V[\x92\x91PPV[`\0\x81\x90P\x91\x90PV[`\0a\x04\xCAa\x04\xC5a\x04\xC0\x84a\x02\x1FV[a\x04\xA5V[a\x02\x1FV[\x90P\x91\x90PV[`\0a\x04\xDC\x82a\x04\xAFV[\x90P\x91\x90PV[`\0a\x04\xEE\x82a\x04\xD1V[\x90P\x91\x90PV[a\x04\xFE\x81a\x04\xE3V[\x82RPPV[`\0` \x82\x01\x90Pa\x05\x19`\0\x83\x01\x84a\x04\xF5V[\x92\x91PPV[a\x05(\x81a\x02?V[\x82RPPV[a\x057\x81a\x02}V[\x82RPPV[a\x05F\x81a\x02\xB3V[\x82RPPV[`\0\x82\x82R` \x82\x01\x90P\x92\x91PPV[\x82\x81\x837`\0\x83\x83\x01RPPPV[`\0`\x1F\x19`\x1F\x83\x01\x16\x90P\x91\x90PV[`\0a\x05\x89\x83\x85a\x05LV[\x93Pa\x05\x96\x83\x85\x84a\x05]V[a\x05\x9F\x83a\x05lV[\x84\x01\x90P\x93\x92PPPV[`\0`\xA0\x82\x01\x90Pa\x05\xBF`\0\x83\x01\x89a\x05\x1FV[a\x05\xCC` \x83\x01\x88a\x05.V[a\x05\xD9`@\x83\x01\x87a\x05=V[\x81\x81\x03``\x83\x01Ra\x05\xEC\x81\x85\x87a\x05}V[\x90Pa\x05\xFB`\x80\x83\x01\x84a\x05\x1FV[\x97\x96PPPPPPPV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0`\0R`\x12`\x04R`$`\0\xFD[`\0a\x06@\x82a\x02}V[\x91Pa\x06K\x83a\x02}V[\x92P\x82a\x06[Wa\x06Za\x06\x06V[[\x82\x82\x04\x90P\x92\x91PPV[`\0`@\x82\x01\x90P\x81\x81\x03`\0\x83\x01Ra\x06\x81\x81\x86\x88a\x05}V[\x90P\x81\x81\x03` \x83\x01Ra\x06\x96\x81\x84\x86a\x05}V[\x90P\x95\x94PPPPPV[a\x06\xAA\x81a\x04oV[\x81\x14a\x06\xB5W`\0\x80\xFD[PV[`\0\x81Q\x90Pa\x06\xC7\x81a\x06\xA1V[\x92\x91PPV[`\0` \x82\x84\x03\x12\x15a\x06\xE3Wa\x06\xE2a\x02\x15V[[`\0a\x06\xF1\x84\x82\x85\x01a\x06\xB8V[\x91PP\x92\x91PPV\xFE\xA2dipfsX\"\x12 Jt\xEDz\\9\xA0\xD04\x9B\x90T\x87\xB1\xBE\xB3?\xBAgS$\xCD@\xCC\xAF\xE7\x9F,\x8A\xE9C\xB9dsolcC\0\x08\r\x003";
    /// The bytecode of the contract.
    pub static MINTATTACKCONTRACT_BYTECODE: ::ethers::core::types::Bytes = ::ethers::core::types::Bytes::from_static(
        __BYTECODE,
    );
    pub struct MintAttackContract<M>(::ethers::contract::Contract<M>);
    impl<M> ::core::clone::Clone for MintAttackContract<M> {
        fn clone(&self) -> Self {
            Self(::core::clone::Clone::clone(&self.0))
        }
    }
    impl<M> ::core::ops::Deref for MintAttackContract<M> {
        type Target = ::ethers::contract::Contract<M>;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl<M> ::core::ops::DerefMut for MintAttackContract<M> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }
    impl<M> ::core::fmt::Debug for MintAttackContract<M> {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            f.debug_tuple(::core::stringify!(MintAttackContract))
                .field(&self.address())
                .finish()
        }
    }
    impl<M: ::ethers::providers::Middleware> MintAttackContract<M> {
        /// Creates a new contract instance with the specified `ethers` client at
        /// `address`. The contract derefs to a `ethers::Contract` object.
        pub fn new<T: Into<::ethers::core::types::Address>>(
            address: T,
            client: ::std::sync::Arc<M>,
        ) -> Self {
            Self(
                ::ethers::contract::Contract::new(
                    address.into(),
                    MINTATTACKCONTRACT_ABI.clone(),
                    client,
                ),
            )
        }
        /// Constructs the general purpose `Deployer` instance based on the provided constructor arguments and sends it.
        /// Returns a new instance of a deployer that returns an instance of this contract after sending the transaction
        ///
        /// Notes:
        /// - If there are no constructor arguments, you should pass `()` as the argument.
        /// - The default poll duration is 7 seconds.
        /// - The default number of confirmations is 1 block.
        ///
        ///
        /// # Example
        ///
        /// Generate contract bindings with `abigen!` and deploy a new contract instance.
        ///
        /// *Note*: this requires a `bytecode` and `abi` object in the `greeter.json` artifact.
        ///
        /// ```ignore
        /// # async fn deploy<M: ethers::providers::Middleware>(client: ::std::sync::Arc<M>) {
        ///     abigen!(Greeter, "../greeter.json");
        ///
        ///    let greeter_contract = Greeter::deploy(client, "Hello world!".to_string()).unwrap().send().await.unwrap();
        ///    let msg = greeter_contract.greet().call().await.unwrap();
        /// # }
        /// ```
        pub fn deploy<T: ::ethers::core::abi::Tokenize>(
            client: ::std::sync::Arc<M>,
            constructor_args: T,
        ) -> ::core::result::Result<
            ::ethers::contract::builders::ContractDeployer<M, Self>,
            ::ethers::contract::ContractError<M>,
        > {
            let factory = ::ethers::contract::ContractFactory::new(
                MINTATTACKCONTRACT_ABI.clone(),
                MINTATTACKCONTRACT_BYTECODE.clone().into(),
                client,
            );
            let deployer = factory.deploy(constructor_args)?;
            let deployer = ::ethers::contract::ContractDeployer::new(deployer);
            Ok(deployer)
        }
        ///Calls the contract's `mintingContract` (0xd2f6f67d) function
        pub fn minting_contract(
            &self,
        ) -> ::ethers::contract::builders::ContractCall<
            M,
            ::ethers::core::types::Address,
        > {
            self.0
                .method_hash([210, 246, 246, 125], ())
                .expect("method not found (this should never happen)")
        }
        ///Calls the contract's `passThroughBurn` (0xcbac7ff6) function
        pub fn pass_through_burn(
            &self,
            destination: ::ethers::core::types::Bytes,
            data: ::ethers::core::types::Bytes,
        ) -> ::ethers::contract::builders::ContractCall<M, bool> {
            self.0
                .method_hash([203, 172, 127, 246], (destination, data))
                .expect("method not found (this should never happen)")
        }
        ///Calls the contract's `passThroughMint` (0x03568012) function
        pub fn pass_through_mint(
            &self,
            destination: ::ethers::core::types::Address,
            amount: ::ethers::core::types::U256,
            bitcoin_block_height: u32,
            metadata: ::ethers::core::types::Bytes,
            refund_address: ::ethers::core::types::Address,
        ) -> ::ethers::contract::builders::ContractCall<M, ()> {
            self.0
                .method_hash(
                    [3, 86, 128, 18],
                    (destination, amount, bitcoin_block_height, metadata, refund_address),
                )
                .expect("method not found (this should never happen)")
        }
    }
    impl<M: ::ethers::providers::Middleware> From<::ethers::contract::Contract<M>>
    for MintAttackContract<M> {
        fn from(contract: ::ethers::contract::Contract<M>) -> Self {
            Self::new(contract.address(), contract.client())
        }
    }
    ///Container type for all input parameters for the `mintingContract` function with signature `mintingContract()` and selector `0xd2f6f67d`
    #[derive(
        Clone,
        ::ethers::contract::EthCall,
        ::ethers::contract::EthDisplay,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash
    )]
    #[ethcall(name = "mintingContract", abi = "mintingContract()")]
    pub struct MintingContractCall;
    ///Container type for all input parameters for the `passThroughBurn` function with signature `passThroughBurn(bytes,bytes)` and selector `0xcbac7ff6`
    #[derive(
        Clone,
        ::ethers::contract::EthCall,
        ::ethers::contract::EthDisplay,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash
    )]
    #[ethcall(name = "passThroughBurn", abi = "passThroughBurn(bytes,bytes)")]
    pub struct PassThroughBurnCall {
        pub destination: ::ethers::core::types::Bytes,
        pub data: ::ethers::core::types::Bytes,
    }
    ///Container type for all input parameters for the `passThroughMint` function with signature `passThroughMint(address,uint256,uint32,bytes,address)` and selector `0x03568012`
    #[derive(
        Clone,
        ::ethers::contract::EthCall,
        ::ethers::contract::EthDisplay,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash
    )]
    #[ethcall(
        name = "passThroughMint",
        abi = "passThroughMint(address,uint256,uint32,bytes,address)"
    )]
    pub struct PassThroughMintCall {
        pub destination: ::ethers::core::types::Address,
        pub amount: ::ethers::core::types::U256,
        pub bitcoin_block_height: u32,
        pub metadata: ::ethers::core::types::Bytes,
        pub refund_address: ::ethers::core::types::Address,
    }
    ///Container type for all of the contract's call
    #[derive(Clone, ::ethers::contract::EthAbiType, Debug, PartialEq, Eq, Hash)]
    pub enum MintAttackContractCalls {
        MintingContract(MintingContractCall),
        PassThroughBurn(PassThroughBurnCall),
        PassThroughMint(PassThroughMintCall),
    }
    impl ::ethers::core::abi::AbiDecode for MintAttackContractCalls {
        fn decode(
            data: impl AsRef<[u8]>,
        ) -> ::core::result::Result<Self, ::ethers::core::abi::AbiError> {
            let data = data.as_ref();
            if let Ok(decoded) = <MintingContractCall as ::ethers::core::abi::AbiDecode>::decode(
                data,
            ) {
                return Ok(Self::MintingContract(decoded));
            }
            if let Ok(decoded) = <PassThroughBurnCall as ::ethers::core::abi::AbiDecode>::decode(
                data,
            ) {
                return Ok(Self::PassThroughBurn(decoded));
            }
            if let Ok(decoded) = <PassThroughMintCall as ::ethers::core::abi::AbiDecode>::decode(
                data,
            ) {
                return Ok(Self::PassThroughMint(decoded));
            }
            Err(::ethers::core::abi::Error::InvalidData.into())
        }
    }
    impl ::ethers::core::abi::AbiEncode for MintAttackContractCalls {
        fn encode(self) -> Vec<u8> {
            match self {
                Self::MintingContract(element) => {
                    ::ethers::core::abi::AbiEncode::encode(element)
                }
                Self::PassThroughBurn(element) => {
                    ::ethers::core::abi::AbiEncode::encode(element)
                }
                Self::PassThroughMint(element) => {
                    ::ethers::core::abi::AbiEncode::encode(element)
                }
            }
        }
    }
    impl ::core::fmt::Display for MintAttackContractCalls {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            match self {
                Self::MintingContract(element) => ::core::fmt::Display::fmt(element, f),
                Self::PassThroughBurn(element) => ::core::fmt::Display::fmt(element, f),
                Self::PassThroughMint(element) => ::core::fmt::Display::fmt(element, f),
            }
        }
    }
    impl ::core::convert::From<MintingContractCall> for MintAttackContractCalls {
        fn from(value: MintingContractCall) -> Self {
            Self::MintingContract(value)
        }
    }
    impl ::core::convert::From<PassThroughBurnCall> for MintAttackContractCalls {
        fn from(value: PassThroughBurnCall) -> Self {
            Self::PassThroughBurn(value)
        }
    }
    impl ::core::convert::From<PassThroughMintCall> for MintAttackContractCalls {
        fn from(value: PassThroughMintCall) -> Self {
            Self::PassThroughMint(value)
        }
    }
    ///Container type for all return fields from the `mintingContract` function with signature `mintingContract()` and selector `0xd2f6f67d`
    #[derive(
        Clone,
        ::ethers::contract::EthAbiType,
        ::ethers::contract::EthAbiCodec,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash
    )]
    pub struct MintingContractReturn(pub ::ethers::core::types::Address);
    ///Container type for all return fields from the `passThroughBurn` function with signature `passThroughBurn(bytes,bytes)` and selector `0xcbac7ff6`
    #[derive(
        Clone,
        ::ethers::contract::EthAbiType,
        ::ethers::contract::EthAbiCodec,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash
    )]
    pub struct PassThroughBurnReturn {
        pub success: bool,
    }
}
