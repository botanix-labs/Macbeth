pub use multi_mint_helper_contract::*;
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
pub mod multi_mint_helper_contract {
    #[allow(deprecated)]
    /// Returns the ABI definition for the MultiMintHelperContract smart contract.
    ///
    /// The ABI describes the contract's constructor and functions, including their input and output types. This is used for encoding and decoding contract calls and events.
    ///
    /// # Examples
    ///
    /// ```
    /// let abi = __abi();
    /// assert!(abi.constructor.is_some());
    /// assert!(abi.functions.contains_key("multiMintTwo"));
    /// ```
    fn __abi() -> ::ethers::core::abi::Abi {
        ::ethers::core::abi::ethabi::Contract {
            constructor: ::core::option::Option::Some(::ethers::core::abi::ethabi::Constructor {
                inputs: ::std::vec![
                    ::ethers::core::abi::ethabi::Param {
                        name: ::std::borrow::ToOwned::to_owned("_mintingContract"),
                        kind: ::ethers::core::abi::ethabi::ParamType::Address,
                        internal_type: ::core::option::Option::Some(
                            ::std::borrow::ToOwned::to_owned("address"),
                        ),
                    },
                ],
            }),
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
                                        ::std::borrow::ToOwned::to_owned("address"),
                                    ),
                                },
                            ],
                            constant: ::core::option::Option::None,
                            state_mutability: ::ethers::core::abi::ethabi::StateMutability::View,
                        },
                    ],
                ),
                (
                    ::std::borrow::ToOwned::to_owned("multiMintTwo"),
                    ::std::vec![
                        ::ethers::core::abi::ethabi::Function {
                            name: ::std::borrow::ToOwned::to_owned("multiMintTwo"),
                            inputs: ::std::vec![
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("destination1"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Address,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("address"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("amount1"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Uint(
                                        256usize,
                                    ),
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("uint256"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned(
                                        "bitcoinBlockHeight1",
                                    ),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Uint(32usize),
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("uint32"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("metadata1"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Bytes,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("bytes"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("refundAddress1"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Address,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("address"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("destination2"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Address,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("address"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("amount2"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Uint(
                                        256usize,
                                    ),
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("uint256"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned(
                                        "bitcoinBlockHeight2",
                                    ),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Uint(32usize),
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("uint32"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("metadata2"),
                                    kind: ::ethers::core::abi::ethabi::ParamType::Bytes,
                                    internal_type: ::core::option::Option::Some(
                                        ::std::borrow::ToOwned::to_owned("bytes"),
                                    ),
                                },
                                ::ethers::core::abi::ethabi::Param {
                                    name: ::std::borrow::ToOwned::to_owned("refundAddress2"),
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
    pub static MULTIMINTHELPERCONTRACT_ABI: ::ethers::contract::Lazy<
        ::ethers::core::abi::Abi,
    > = ::ethers::contract::Lazy::new(__abi);
    #[rustfmt::skip]
    const __BYTECODE: &[u8] = b"`\xA0`@R4\x80\x15a\0\x10W`\0\x80\xFD[P`@Qa\nz8\x03\x80a\nz\x839\x81\x81\x01`@R\x81\x01\x90a\x002\x91\x90a\0\xCFV[\x80s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16`\x80\x81s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x81RPPPa\0\xFCV[`\0\x80\xFD[`\0s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[`\0a\0\x9C\x82a\0qV[\x90P\x91\x90PV[a\0\xAC\x81a\0\x91V[\x81\x14a\0\xB7W`\0\x80\xFD[PV[`\0\x81Q\x90Pa\0\xC9\x81a\0\xA3V[\x92\x91PPV[`\0` \x82\x84\x03\x12\x15a\0\xE5Wa\0\xE4a\0lV[[`\0a\0\xF3\x84\x82\x85\x01a\0\xBAV[\x91PP\x92\x91PPV[`\x80Qa\tVa\x01$`\09`\0\x81\x81`w\x01R\x81\x81a\x018\x01Ra\x02\xA0\x01Ra\tV`\0\xF3\xFE`\x80`@R4\x80\x15a\0\x10W`\0\x80\xFD[P`\x046\x10a\x006W`\x005`\xE0\x1C\x80c\xD2\xF6\xF6}\x14a\0;W\x80c\xE3\x9B\xFEF\x14a\0YW[`\0\x80\xFD[a\0Ca\0uV[`@Qa\0P\x91\x90a\x03\xBAV[`@Q\x80\x91\x03\x90\xF3[a\0s`\x04\x806\x03\x81\x01\x90a\0n\x91\x90a\x05\xCDV[a\0\x99V[\0[\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81V[`\0\x7F_\xE0?E\xC4*\x83|\x0B\xED_)\x7F\xB7\x88y\xCB\x15>\x06\x9C9\x9Bw\xBF\xCCt\xC1\xDD\xB1\xD0^\x8B\x8B\x8B\x8B\x8B`@Q`$\x01a\0\xD5\x95\x94\x93\x92\x91\x90a\x07\x8AV[`@Q` \x81\x83\x03\x03\x81R\x90`@R\x90{\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x19\x16` \x82\x01\x80Q{\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x83\x81\x83\x16\x17\x83RPPPP\x90P`\0\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x82`@Qa\x01{\x91\x90a\x08 V[`\0`@Q\x80\x83\x03\x81`\0\x86Z\xF1\x91PP=\x80`\0\x81\x14a\x01\xB8W`@Q\x91P`\x1F\x19`?=\x01\x16\x82\x01`@R=\x82R=`\0` \x84\x01>a\x01\xBDV[``\x91P[PP\x90P\x80a\x02\x01W`@Q\x7F\x08\xC3y\xA0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\x01\xF8\x90a\x08\x94V[`@Q\x80\x91\x03\x90\xFD[`\0\x7F_\xE0?E\xC4*\x83|\x0B\xED_)\x7F\xB7\x88y\xCB\x15>\x06\x9C9\x9Bw\xBF\xCCt\xC1\xDD\xB1\xD0^\x88\x88\x88\x88\x88`@Q`$\x01a\x02=\x95\x94\x93\x92\x91\x90a\x07\x8AV[`@Q` \x81\x83\x03\x03\x81R\x90`@R\x90{\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x19\x16` \x82\x01\x80Q{\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x83\x81\x83\x16\x17\x83RPPPP\x90P`\0\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x82`@Qa\x02\xE3\x91\x90a\x08 V[`\0`@Q\x80\x83\x03\x81`\0\x86Z\xF1\x91PP=\x80`\0\x81\x14a\x03 W`@Q\x91P`\x1F\x19`?=\x01\x16\x82\x01`@R=\x82R=`\0` \x84\x01>a\x03%V[``\x91P[PP\x90P\x80a\x03iW`@Q\x7F\x08\xC3y\xA0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\x03`\x90a\t\0V[`@Q\x80\x91\x03\x90\xFD[PPPPPPPPPPPPPPV[`\0s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[`\0a\x03\xA4\x82a\x03yV[\x90P\x91\x90PV[a\x03\xB4\x81a\x03\x99V[\x82RPPV[`\0` \x82\x01\x90Pa\x03\xCF`\0\x83\x01\x84a\x03\xABV[\x92\x91PPV[`\0`@Q\x90P\x90V[`\0\x80\xFD[`\0\x80\xFD[a\x03\xF2\x81a\x03\x99V[\x81\x14a\x03\xFDW`\0\x80\xFD[PV[`\0\x815\x90Pa\x04\x0F\x81a\x03\xE9V[\x92\x91PPV[`\0\x81\x90P\x91\x90PV[a\x04(\x81a\x04\x15V[\x81\x14a\x043W`\0\x80\xFD[PV[`\0\x815\x90Pa\x04E\x81a\x04\x1FV[\x92\x91PPV[`\0c\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[a\x04d\x81a\x04KV[\x81\x14a\x04oW`\0\x80\xFD[PV[`\0\x815\x90Pa\x04\x81\x81a\x04[V[\x92\x91PPV[`\0\x80\xFD[`\0\x80\xFD[`\0`\x1F\x19`\x1F\x83\x01\x16\x90P\x91\x90PV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0`\0R`A`\x04R`$`\0\xFD[a\x04\xDA\x82a\x04\x91V[\x81\x01\x81\x81\x10g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x17\x15a\x04\xF9Wa\x04\xF8a\x04\xA2V[[\x80`@RPPPV[`\0a\x05\x0Ca\x03\xD5V[\x90Pa\x05\x18\x82\x82a\x04\xD1V[\x91\x90PV[`\0g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a\x058Wa\x057a\x04\xA2V[[a\x05A\x82a\x04\x91V[\x90P` \x81\x01\x90P\x91\x90PV[\x82\x81\x837`\0\x83\x83\x01RPPPV[`\0a\x05pa\x05k\x84a\x05\x1DV[a\x05\x02V[\x90P\x82\x81R` \x81\x01\x84\x84\x84\x01\x11\x15a\x05\x8CWa\x05\x8Ba\x04\x8CV[[a\x05\x97\x84\x82\x85a\x05NV[P\x93\x92PPPV[`\0\x82`\x1F\x83\x01\x12a\x05\xB4Wa\x05\xB3a\x04\x87V[[\x815a\x05\xC4\x84\x82` \x86\x01a\x05]V[\x91PP\x92\x91PPV[`\0\x80`\0\x80`\0\x80`\0\x80`\0\x80a\x01@\x8B\x8D\x03\x12\x15a\x05\xF1Wa\x05\xF0a\x03\xDFV[[`\0a\x05\xFF\x8D\x82\x8E\x01a\x04\0V[\x9APP` a\x06\x10\x8D\x82\x8E\x01a\x046V[\x99PP`@a\x06!\x8D\x82\x8E\x01a\x04rV[\x98PP``\x8B\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x06BWa\x06Aa\x03\xE4V[[a\x06N\x8D\x82\x8E\x01a\x05\x9FV[\x97PP`\x80a\x06_\x8D\x82\x8E\x01a\x04\0V[\x96PP`\xA0a\x06p\x8D\x82\x8E\x01a\x04\0V[\x95PP`\xC0a\x06\x81\x8D\x82\x8E\x01a\x046V[\x94PP`\xE0a\x06\x92\x8D\x82\x8E\x01a\x04rV[\x93PPa\x01\0\x8B\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x06\xB4Wa\x06\xB3a\x03\xE4V[[a\x06\xC0\x8D\x82\x8E\x01a\x05\x9FV[\x92PPa\x01 a\x06\xD2\x8D\x82\x8E\x01a\x04\0V[\x91PP\x92\x95\x98\x9B\x91\x94\x97\x9AP\x92\x95\x98PV[a\x06\xED\x81a\x04\x15V[\x82RPPV[a\x06\xFC\x81a\x04KV[\x82RPPV[`\0\x81Q\x90P\x91\x90PV[`\0\x82\x82R` \x82\x01\x90P\x92\x91PPV[`\0[\x83\x81\x10\x15a\x07<W\x80\x82\x01Q\x81\x84\x01R` \x81\x01\x90Pa\x07!V[\x83\x81\x11\x15a\x07KW`\0\x84\x84\x01R[PPPPV[`\0a\x07\\\x82a\x07\x02V[a\x07f\x81\x85a\x07\rV[\x93Pa\x07v\x81\x85` \x86\x01a\x07\x1EV[a\x07\x7F\x81a\x04\x91V[\x84\x01\x91PP\x92\x91PPV[`\0`\xA0\x82\x01\x90Pa\x07\x9F`\0\x83\x01\x88a\x03\xABV[a\x07\xAC` \x83\x01\x87a\x06\xE4V[a\x07\xB9`@\x83\x01\x86a\x06\xF3V[\x81\x81\x03``\x83\x01Ra\x07\xCB\x81\x85a\x07QV[\x90Pa\x07\xDA`\x80\x83\x01\x84a\x03\xABV[\x96\x95PPPPPPV[`\0\x81\x90P\x92\x91PPV[`\0a\x07\xFA\x82a\x07\x02V[a\x08\x04\x81\x85a\x07\xE4V[\x93Pa\x08\x14\x81\x85` \x86\x01a\x07\x1EV[\x80\x84\x01\x91PP\x92\x91PPV[`\0a\x08,\x82\x84a\x07\xEFV[\x91P\x81\x90P\x92\x91PPV[`\0\x82\x82R` \x82\x01\x90P\x92\x91PPV[\x7FFirst mint call failed\0\0\0\0\0\0\0\0\0\0`\0\x82\x01RPV[`\0a\x08~`\x16\x83a\x087V[\x91Pa\x08\x89\x82a\x08HV[` \x82\x01\x90P\x91\x90PV[`\0` \x82\x01\x90P\x81\x81\x03`\0\x83\x01Ra\x08\xAD\x81a\x08qV[\x90P\x91\x90PV[\x7FSecond mint call failed\0\0\0\0\0\0\0\0\0`\0\x82\x01RPV[`\0a\x08\xEA`\x17\x83a\x087V[\x91Pa\x08\xF5\x82a\x08\xB4V[` \x82\x01\x90P\x91\x90PV[`\0` \x82\x01\x90P\x81\x81\x03`\0\x83\x01Ra\t\x19\x81a\x08\xDDV[\x90P\x91\x90PV\xFE\xA2dipfsX\"\x12 \x01\xAC:\x8Fb\xE1\xFE\x11\x1A\x94.\xDE\x98<8\x86\xF4j\x87.:e\xA9o:\x0B@\nWz\x97\x8FdsolcC\0\x08\r\x003";
    /// The bytecode of the contract.
    pub static MULTIMINTHELPERCONTRACT_BYTECODE: ::ethers::core::types::Bytes = ::ethers::core::types::Bytes::from_static(
        __BYTECODE,
    );
    #[rustfmt::skip]
    const __DEPLOYED_BYTECODE: &[u8] = b"`\x80`@R4\x80\x15a\0\x10W`\0\x80\xFD[P`\x046\x10a\x006W`\x005`\xE0\x1C\x80c\xD2\xF6\xF6}\x14a\0;W\x80c\xE3\x9B\xFEF\x14a\0YW[`\0\x80\xFD[a\0Ca\0uV[`@Qa\0P\x91\x90a\x03\xBAV[`@Q\x80\x91\x03\x90\xF3[a\0s`\x04\x806\x03\x81\x01\x90a\0n\x91\x90a\x05\xCDV[a\0\x99V[\0[\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81V[`\0\x7F_\xE0?E\xC4*\x83|\x0B\xED_)\x7F\xB7\x88y\xCB\x15>\x06\x9C9\x9Bw\xBF\xCCt\xC1\xDD\xB1\xD0^\x8B\x8B\x8B\x8B\x8B`@Q`$\x01a\0\xD5\x95\x94\x93\x92\x91\x90a\x07\x8AV[`@Q` \x81\x83\x03\x03\x81R\x90`@R\x90{\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x19\x16` \x82\x01\x80Q{\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x83\x81\x83\x16\x17\x83RPPPP\x90P`\0\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x82`@Qa\x01{\x91\x90a\x08 V[`\0`@Q\x80\x83\x03\x81`\0\x86Z\xF1\x91PP=\x80`\0\x81\x14a\x01\xB8W`@Q\x91P`\x1F\x19`?=\x01\x16\x82\x01`@R=\x82R=`\0` \x84\x01>a\x01\xBDV[``\x91P[PP\x90P\x80a\x02\x01W`@Q\x7F\x08\xC3y\xA0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\x01\xF8\x90a\x08\x94V[`@Q\x80\x91\x03\x90\xFD[`\0\x7F_\xE0?E\xC4*\x83|\x0B\xED_)\x7F\xB7\x88y\xCB\x15>\x06\x9C9\x9Bw\xBF\xCCt\xC1\xDD\xB1\xD0^\x88\x88\x88\x88\x88`@Q`$\x01a\x02=\x95\x94\x93\x92\x91\x90a\x07\x8AV[`@Q` \x81\x83\x03\x03\x81R\x90`@R\x90{\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x19\x16` \x82\x01\x80Q{\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x83\x81\x83\x16\x17\x83RPPPP\x90P`\0\x7F\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x16\x82`@Qa\x02\xE3\x91\x90a\x08 V[`\0`@Q\x80\x83\x03\x81`\0\x86Z\xF1\x91PP=\x80`\0\x81\x14a\x03 W`@Q\x91P`\x1F\x19`?=\x01\x16\x82\x01`@R=\x82R=`\0` \x84\x01>a\x03%V[``\x91P[PP\x90P\x80a\x03iW`@Q\x7F\x08\xC3y\xA0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x81R`\x04\x01a\x03`\x90a\t\0V[`@Q\x80\x91\x03\x90\xFD[PPPPPPPPPPPPPPV[`\0s\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[`\0a\x03\xA4\x82a\x03yV[\x90P\x91\x90PV[a\x03\xB4\x81a\x03\x99V[\x82RPPV[`\0` \x82\x01\x90Pa\x03\xCF`\0\x83\x01\x84a\x03\xABV[\x92\x91PPV[`\0`@Q\x90P\x90V[`\0\x80\xFD[`\0\x80\xFD[a\x03\xF2\x81a\x03\x99V[\x81\x14a\x03\xFDW`\0\x80\xFD[PV[`\0\x815\x90Pa\x04\x0F\x81a\x03\xE9V[\x92\x91PPV[`\0\x81\x90P\x91\x90PV[a\x04(\x81a\x04\x15V[\x81\x14a\x043W`\0\x80\xFD[PV[`\0\x815\x90Pa\x04E\x81a\x04\x1FV[\x92\x91PPV[`\0c\xFF\xFF\xFF\xFF\x82\x16\x90P\x91\x90PV[a\x04d\x81a\x04KV[\x81\x14a\x04oW`\0\x80\xFD[PV[`\0\x815\x90Pa\x04\x81\x81a\x04[V[\x92\x91PPV[`\0\x80\xFD[`\0\x80\xFD[`\0`\x1F\x19`\x1F\x83\x01\x16\x90P\x91\x90PV[\x7FNH{q\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0`\0R`A`\x04R`$`\0\xFD[a\x04\xDA\x82a\x04\x91V[\x81\x01\x81\x81\x10g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x17\x15a\x04\xF9Wa\x04\xF8a\x04\xA2V[[\x80`@RPPPV[`\0a\x05\x0Ca\x03\xD5V[\x90Pa\x05\x18\x82\x82a\x04\xD1V[\x91\x90PV[`\0g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x82\x11\x15a\x058Wa\x057a\x04\xA2V[[a\x05A\x82a\x04\x91V[\x90P` \x81\x01\x90P\x91\x90PV[\x82\x81\x837`\0\x83\x83\x01RPPPV[`\0a\x05pa\x05k\x84a\x05\x1DV[a\x05\x02V[\x90P\x82\x81R` \x81\x01\x84\x84\x84\x01\x11\x15a\x05\x8CWa\x05\x8Ba\x04\x8CV[[a\x05\x97\x84\x82\x85a\x05NV[P\x93\x92PPPV[`\0\x82`\x1F\x83\x01\x12a\x05\xB4Wa\x05\xB3a\x04\x87V[[\x815a\x05\xC4\x84\x82` \x86\x01a\x05]V[\x91PP\x92\x91PPV[`\0\x80`\0\x80`\0\x80`\0\x80`\0\x80a\x01@\x8B\x8D\x03\x12\x15a\x05\xF1Wa\x05\xF0a\x03\xDFV[[`\0a\x05\xFF\x8D\x82\x8E\x01a\x04\0V[\x9APP` a\x06\x10\x8D\x82\x8E\x01a\x046V[\x99PP`@a\x06!\x8D\x82\x8E\x01a\x04rV[\x98PP``\x8B\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x06BWa\x06Aa\x03\xE4V[[a\x06N\x8D\x82\x8E\x01a\x05\x9FV[\x97PP`\x80a\x06_\x8D\x82\x8E\x01a\x04\0V[\x96PP`\xA0a\x06p\x8D\x82\x8E\x01a\x04\0V[\x95PP`\xC0a\x06\x81\x8D\x82\x8E\x01a\x046V[\x94PP`\xE0a\x06\x92\x8D\x82\x8E\x01a\x04rV[\x93PPa\x01\0\x8B\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x06\xB4Wa\x06\xB3a\x03\xE4V[[a\x06\xC0\x8D\x82\x8E\x01a\x05\x9FV[\x92PPa\x01 a\x06\xD2\x8D\x82\x8E\x01a\x04\0V[\x91PP\x92\x95\x98\x9B\x91\x94\x97\x9AP\x92\x95\x98PV[a\x06\xED\x81a\x04\x15V[\x82RPPV[a\x06\xFC\x81a\x04KV[\x82RPPV[`\0\x81Q\x90P\x91\x90PV[`\0\x82\x82R` \x82\x01\x90P\x92\x91PPV[`\0[\x83\x81\x10\x15a\x07<W\x80\x82\x01Q\x81\x84\x01R` \x81\x01\x90Pa\x07!V[\x83\x81\x11\x15a\x07KW`\0\x84\x84\x01R[PPPPV[`\0a\x07\\\x82a\x07\x02V[a\x07f\x81\x85a\x07\rV[\x93Pa\x07v\x81\x85` \x86\x01a\x07\x1EV[a\x07\x7F\x81a\x04\x91V[\x84\x01\x91PP\x92\x91PPV[`\0`\xA0\x82\x01\x90Pa\x07\x9F`\0\x83\x01\x88a\x03\xABV[a\x07\xAC` \x83\x01\x87a\x06\xE4V[a\x07\xB9`@\x83\x01\x86a\x06\xF3V[\x81\x81\x03``\x83\x01Ra\x07\xCB\x81\x85a\x07QV[\x90Pa\x07\xDA`\x80\x83\x01\x84a\x03\xABV[\x96\x95PPPPPPV[`\0\x81\x90P\x92\x91PPV[`\0a\x07\xFA\x82a\x07\x02V[a\x08\x04\x81\x85a\x07\xE4V[\x93Pa\x08\x14\x81\x85` \x86\x01a\x07\x1EV[\x80\x84\x01\x91PP\x92\x91PPV[`\0a\x08,\x82\x84a\x07\xEFV[\x91P\x81\x90P\x92\x91PPV[`\0\x82\x82R` \x82\x01\x90P\x92\x91PPV[\x7FFirst mint call failed\0\0\0\0\0\0\0\0\0\0`\0\x82\x01RPV[`\0a\x08~`\x16\x83a\x087V[\x91Pa\x08\x89\x82a\x08HV[` \x82\x01\x90P\x91\x90PV[`\0` \x82\x01\x90P\x81\x81\x03`\0\x83\x01Ra\x08\xAD\x81a\x08qV[\x90P\x91\x90PV[\x7FSecond mint call failed\0\0\0\0\0\0\0\0\0`\0\x82\x01RPV[`\0a\x08\xEA`\x17\x83a\x087V[\x91Pa\x08\xF5\x82a\x08\xB4V[` \x82\x01\x90P\x91\x90PV[`\0` \x82\x01\x90P\x81\x81\x03`\0\x83\x01Ra\t\x19\x81a\x08\xDDV[\x90P\x91\x90PV\xFE\xA2dipfsX\"\x12 \x01\xAC:\x8Fb\xE1\xFE\x11\x1A\x94.\xDE\x98<8\x86\xF4j\x87.:e\xA9o:\x0B@\nWz\x97\x8FdsolcC\0\x08\r\x003";
    /// The deployed bytecode of the contract.
    pub static MULTIMINTHELPERCONTRACT_DEPLOYED_BYTECODE: ::ethers::core::types::Bytes = ::ethers::core::types::Bytes::from_static(
        __DEPLOYED_BYTECODE,
    );
    pub struct MultiMintHelperContract<M>(::ethers::contract::Contract<M>);
    impl<M> ::core::clone::Clone for MultiMintHelperContract<M> {
        /// Creates a copy of the contract instance.
        ///
        /// The cloned instance shares the same contract address and client as the original.
        fn clone(&self) -> Self {
            Self(::core::clone::Clone::clone(&self.0))
        }
    }
    impl<M> ::core::ops::Deref for MultiMintHelperContract<M> {
        type Target = ::ethers::contract::Contract<M>;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl<M> ::core::ops::DerefMut for MultiMintHelperContract<M> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }
    impl<M> ::core::fmt::Debug for MultiMintHelperContract<M> {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            f.debug_tuple(::core::stringify!(MultiMintHelperContract))
                .field(&self.address())
                .finish()
        }
    }
    impl<M: ::ethers::providers::Middleware> MultiMintHelperContract<M> {
        /// Creates a new contract instance with the specified `ethers` client at
        /// Creates a new instance of the MultiMintHelperContract at the specified address.
        ///
        /// # Examples
        ///
        /// ```
        /// use ethers::providers::{Provider, Http};
        /// use std::sync::Arc;
        /// let provider = Provider::<Http>::try_from("http://localhost:8545").unwrap();
        /// let client = Arc::new(provider);
        /// let contract_address = "0x0000000000000000000000000000000000000000".parse().unwrap();
        /// let contract = MultiMintHelperContract::new(contract_address, client);
        /// ```
        pub fn new<T: Into<::ethers::core::types::Address>>(
            address: T,
            client: ::std::sync::Arc<M>,
        ) -> Self {
            Self(
                ::ethers::contract::Contract::new(
                    address.into(),
                    MULTIMINTHELPERCONTRACT_ABI.clone(),
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
                MULTIMINTHELPERCONTRACT_ABI.clone(),
                MULTIMINTHELPERCONTRACT_BYTECODE.clone().into(),
                client,
            );
            let deployer = factory.deploy(constructor_args)?;
            let deployer = ::ethers::contract::ContractDeployer::new(deployer);
            Ok(deployer)
        }
        /// Returns the address of the associated minting contract.
        ///
        /// # Examples
        ///
        /// ```
        /// let contract = MultiMintHelperContract::new(address, client);
        /// let call = contract.minting_contract();
        /// let minting_address = call.call().await?;
        /// ```
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
        /// Executes two mint operations in a single transaction.
        ///
        /// Calls the contract's `multiMintTwo` function, minting tokens to two separate destinations with specified amounts, Bitcoin block heights, metadata, and refund addresses.
        ///
        /// # Parameters
        /// - `destination_1`: Address to receive the first mint.
        /// - `amount_1`: Amount to mint for the first destination.
        /// - `bitcoin_block_height_1`: Bitcoin block height associated with the first mint.
        /// - `metadata_1`: Arbitrary metadata for the first mint.
        /// - `refund_address_1`: Address to receive any refund from the first mint.
        /// - `destination_2`: Address to receive the second mint.
        /// - `amount_2`: Amount to mint for the second destination.
        /// - `bitcoin_block_height_2`: Bitcoin block height associated with the second mint.
        /// - `metadata_2`: Arbitrary metadata for the second mint.
        /// - `refund_address_2`: Address to receive any refund from the second mint.
        ///
        /// # Returns
        /// A contract call builder for executing the `multiMintTwo` transaction.
        ///
        /// # Examples
        ///
        /// ```
        /// let call = contract.multi_mint_two(
        ///     addr1, amount1, block_height1, metadata1, refund1,
        ///     addr2, amount2, block_height2, metadata2, refund2,
        /// );
        /// // To send the transaction:
        /// // let pending_tx = call.send().await?;
        /// ```
        pub fn multi_mint_two(
            &self,
            destination_1: ::ethers::core::types::Address,
            amount_1: ::ethers::core::types::U256,
            bitcoin_block_height_1: u32,
            metadata_1: ::ethers::core::types::Bytes,
            refund_address_1: ::ethers::core::types::Address,
            destination_2: ::ethers::core::types::Address,
            amount_2: ::ethers::core::types::U256,
            bitcoin_block_height_2: u32,
            metadata_2: ::ethers::core::types::Bytes,
            refund_address_2: ::ethers::core::types::Address,
        ) -> ::ethers::contract::builders::ContractCall<M, ()> {
            self.0
                .method_hash(
                    [227, 155, 254, 70],
                    (
                        destination_1,
                        amount_1,
                        bitcoin_block_height_1,
                        metadata_1,
                        refund_address_1,
                        destination_2,
                        amount_2,
                        bitcoin_block_height_2,
                        metadata_2,
                        refund_address_2,
                    ),
                )
                .expect("method not found (this should never happen)")
        }
    }
    impl<M: ::ethers::providers::Middleware> From<::ethers::contract::Contract<M>>
    for MultiMintHelperContract<M> {
        /// Creates a `MultiMintHelperContract` instance from an existing ethers-rs `Contract`.
        ///
        /// # Examples
        ///
        /// ```
        /// let ethers_contract = Contract::new(address, client, abi);
        /// let helper = MultiMintHelperContract::from(ethers_contract);
        /// ```
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
    ///Container type for all input parameters for the `multiMintTwo` function with signature `multiMintTwo(address,uint256,uint32,bytes,address,address,uint256,uint32,bytes,address)` and selector `0xe39bfe46`
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
        name = "multiMintTwo",
        abi = "multiMintTwo(address,uint256,uint32,bytes,address,address,uint256,uint32,bytes,address)"
    )]
    pub struct MultiMintTwoCall {
        pub destination_1: ::ethers::core::types::Address,
        pub amount_1: ::ethers::core::types::U256,
        pub bitcoin_block_height_1: u32,
        pub metadata_1: ::ethers::core::types::Bytes,
        pub refund_address_1: ::ethers::core::types::Address,
        pub destination_2: ::ethers::core::types::Address,
        pub amount_2: ::ethers::core::types::U256,
        pub bitcoin_block_height_2: u32,
        pub metadata_2: ::ethers::core::types::Bytes,
        pub refund_address_2: ::ethers::core::types::Address,
    }
    ///Container type for all of the contract's call
    #[derive(Clone, ::ethers::contract::EthAbiType, Debug, PartialEq, Eq, Hash)]
    pub enum MultiMintHelperContractCalls {
        MintingContract(MintingContractCall),
        MultiMintTwo(MultiMintTwoCall),
    }
    impl ::ethers::core::abi::AbiDecode for MultiMintHelperContractCalls {
        /// Decodes ABI-encoded call data into a `MultiMintHelperContractCalls` enum variant.
        ///
        /// Attempts to decode the provided data as either a `MintingContractCall` or a `MultiMintTwoCall`.
        /// Returns the corresponding enum variant if successful, or an error if the data does not match either call type.
        ///
        /// # Examples
        ///
        /// ```
        /// let call_data = /* ABI-encoded call data as bytes */;
        /// let decoded = MultiMintHelperContractCalls::decode(call_data);
        /// assert!(decoded.is_ok() || decoded.is_err());
        /// ```
        fn decode(
            data: impl AsRef<[u8]>,
        ) -> ::core::result::Result<Self, ::ethers::core::abi::AbiError> {
            let data = data.as_ref();
            if let Ok(decoded) = <MintingContractCall as ::ethers::core::abi::AbiDecode>::decode(
                data,
            ) {
                return Ok(Self::MintingContract(decoded));
            }
            if let Ok(decoded) = <MultiMintTwoCall as ::ethers::core::abi::AbiDecode>::decode(
                data,
            ) {
                return Ok(Self::MultiMintTwo(decoded));
            }
            Err(::ethers::core::abi::Error::InvalidData.into())
        }
    }
    impl ::ethers::core::abi::AbiEncode for MultiMintHelperContractCalls {
        /// Encodes the enum variant into ABI-compliant bytes for contract interaction.
        ///
        /// # Returns
        ///
        /// A vector of bytes representing the ABI-encoded data of the selected call variant.
        ///
        /// # Examples
        ///
        /// ```
        /// let call = MultiMintHelperContractCalls::MintingContract(MintingContractCall {});
        /// let encoded = call.encode();
        /// assert!(!encoded.is_empty());
        /// ```
        fn encode(self) -> Vec<u8> {
            match self {
                Self::MintingContract(element) => {
                    ::ethers::core::abi::AbiEncode::encode(element)
                }
                Self::MultiMintTwo(element) => {
                    ::ethers::core::abi::AbiEncode::encode(element)
                }
            }
        }
    }
    impl ::core::fmt::Display for MultiMintHelperContractCalls {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            match self {
                Self::MintingContract(element) => ::core::fmt::Display::fmt(element, f),
                Self::MultiMintTwo(element) => ::core::fmt::Display::fmt(element, f),
            }
        }
    }
    impl ::core::convert::From<MintingContractCall> for MultiMintHelperContractCalls {
        fn from(value: MintingContractCall) -> Self {
            Self::MintingContract(value)
        }
    }
    impl ::core::convert::From<MultiMintTwoCall> for MultiMintHelperContractCalls {
        /// Creates a `MultiMintHelperContractCalls` enum variant from a `MultiMintTwoCall` value.
        fn from(value: MultiMintTwoCall) -> Self {
            Self::MultiMintTwo(value)
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
}
