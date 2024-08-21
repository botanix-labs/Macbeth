pub use minting::*;
/// This module was auto-generated with ethers-rs Abigen.
/// More information at: <https://github.com/gakonst/ethers-rs>
#[allow(
    clippy::enum_variant_names,
    clippy::too_many_arguments,
    clippy::upper_case_acronyms,
    clippy::type_complexity,
    dead_code,
    non_camel_case_types
)]
pub mod minting {
    #[allow(deprecated)]
    fn __abi() -> ::ethers::core::abi::Abi {
        ::ethers::core::abi::ethabi::Contract {
            constructor: ::core::option::Option::None,
            functions: ::core::convert::From::from([
                (
                    ::std::borrow::ToOwned::to_owned("SATS_TO_WEI"),
                    ::std::vec![::ethers::core::abi::ethabi::Function {
                        name: ::std::borrow::ToOwned::to_owned("SATS_TO_WEI"),
                        inputs: ::std::vec![],
                        outputs: ::std::vec![::ethers::core::abi::ethabi::Param {
                            name: ::std::string::String::new(),
                            kind: ::ethers::core::abi::ethabi::ParamType::Uint(256usize,),
                            internal_type: ::core::option::Option::Some(
                                ::std::borrow::ToOwned::to_owned("uint256"),
                            ),
                        },],
                        constant: ::core::option::Option::None,
                        state_mutability: ::ethers::core::abi::ethabi::StateMutability::View,
                    },],
                ),
                (
                    ::std::borrow::ToOwned::to_owned("burn"),
                    ::std::vec![::ethers::core::abi::ethabi::Function {
                        name: ::std::borrow::ToOwned::to_owned("burn"),
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
                        outputs: ::std::vec![::ethers::core::abi::ethabi::Param {
                            name: ::std::borrow::ToOwned::to_owned("success"),
                            kind: ::ethers::core::abi::ethabi::ParamType::Bool,
                            internal_type: ::core::option::Option::Some(
                                ::std::borrow::ToOwned::to_owned("bool"),
                            ),
                        },],
                        constant: ::core::option::Option::None,
                        state_mutability: ::ethers::core::abi::ethabi::StateMutability::Payable,
                    },],
                ),
                (
                    ::std::borrow::ToOwned::to_owned("mint"),
                    ::std::vec![::ethers::core::abi::ethabi::Function {
                        name: ::std::borrow::ToOwned::to_owned("mint"),
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
                                kind: ::ethers::core::abi::ethabi::ParamType::Uint(256usize,),
                                internal_type: ::core::option::Option::Some(
                                    ::std::borrow::ToOwned::to_owned("uint256"),
                                ),
                            },
                            ::ethers::core::abi::ethabi::Param {
                                name: ::std::borrow::ToOwned::to_owned("bitcoinBlockHeight",),
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
                    },],
                ),
                (
                    ::std::borrow::ToOwned::to_owned("peginBitcoinBlockHeight"),
                    ::std::vec![::ethers::core::abi::ethabi::Function {
                        name: ::std::borrow::ToOwned::to_owned("peginBitcoinBlockHeight",),
                        inputs: ::std::vec![::ethers::core::abi::ethabi::Param {
                            name: ::std::string::String::new(),
                            kind: ::ethers::core::abi::ethabi::ParamType::Address,
                            internal_type: ::core::option::Option::Some(
                                ::std::borrow::ToOwned::to_owned("address"),
                            ),
                        },],
                        outputs: ::std::vec![::ethers::core::abi::ethabi::Param {
                            name: ::std::string::String::new(),
                            kind: ::ethers::core::abi::ethabi::ParamType::Uint(32usize),
                            internal_type: ::core::option::Option::Some(
                                ::std::borrow::ToOwned::to_owned("uint32"),
                            ),
                        },],
                        constant: ::core::option::Option::None,
                        state_mutability: ::ethers::core::abi::ethabi::StateMutability::View,
                    },],
                ),
            ]),
            events: ::core::convert::From::from([
                (
                    ::std::borrow::ToOwned::to_owned("Burn"),
                    ::std::vec![::ethers::core::abi::ethabi::Event {
                        name: ::std::borrow::ToOwned::to_owned("Burn"),
                        inputs: ::std::vec![
                            ::ethers::core::abi::ethabi::EventParam {
                                name: ::std::borrow::ToOwned::to_owned("account"),
                                kind: ::ethers::core::abi::ethabi::ParamType::Address,
                                indexed: true,
                            },
                            ::ethers::core::abi::ethabi::EventParam {
                                name: ::std::borrow::ToOwned::to_owned("amount"),
                                kind: ::ethers::core::abi::ethabi::ParamType::Uint(256usize,),
                                indexed: false,
                            },
                            ::ethers::core::abi::ethabi::EventParam {
                                name: ::std::borrow::ToOwned::to_owned("destination"),
                                kind: ::ethers::core::abi::ethabi::ParamType::Bytes,
                                indexed: false,
                            },
                            ::ethers::core::abi::ethabi::EventParam {
                                name: ::std::borrow::ToOwned::to_owned("metadata"),
                                kind: ::ethers::core::abi::ethabi::ParamType::Bytes,
                                indexed: false,
                            },
                        ],
                        anonymous: false,
                    },],
                ),
                (
                    ::std::borrow::ToOwned::to_owned("Mint"),
                    ::std::vec![::ethers::core::abi::ethabi::Event {
                        name: ::std::borrow::ToOwned::to_owned("Mint"),
                        inputs: ::std::vec![
                            ::ethers::core::abi::ethabi::EventParam {
                                name: ::std::borrow::ToOwned::to_owned("account"),
                                kind: ::ethers::core::abi::ethabi::ParamType::Address,
                                indexed: true,
                            },
                            ::ethers::core::abi::ethabi::EventParam {
                                name: ::std::borrow::ToOwned::to_owned("amount"),
                                kind: ::ethers::core::abi::ethabi::ParamType::Uint(256usize,),
                                indexed: false,
                            },
                            ::ethers::core::abi::ethabi::EventParam {
                                name: ::std::borrow::ToOwned::to_owned("bitcoinBlockHeight",),
                                kind: ::ethers::core::abi::ethabi::ParamType::Uint(32usize),
                                indexed: false,
                            },
                            ::ethers::core::abi::ethabi::EventParam {
                                name: ::std::borrow::ToOwned::to_owned("metadata"),
                                kind: ::ethers::core::abi::ethabi::ParamType::Bytes,
                                indexed: false,
                            },
                        ],
                        anonymous: false,
                    },],
                ),
                (
                    ::std::borrow::ToOwned::to_owned("MintAmount"),
                    ::std::vec![::ethers::core::abi::ethabi::Event {
                        name: ::std::borrow::ToOwned::to_owned("MintAmount"),
                        inputs: ::std::vec![::ethers::core::abi::ethabi::EventParam {
                            name: ::std::borrow::ToOwned::to_owned("amount"),
                            kind: ::ethers::core::abi::ethabi::ParamType::Uint(256usize,),
                            indexed: true,
                        },],
                        anonymous: false,
                    },],
                ),
            ]),
            errors: ::std::collections::BTreeMap::new(),
            receive: false,
            fallback: false,
        }
    }
    ///The parsed JSON ABI of the contract.
    pub static MINTING_ABI: ::ethers::contract::Lazy<::ethers::core::abi::Abi> =
        ::ethers::contract::Lazy::new(__abi);
    #[rustfmt::skip]
    const __BYTECODE: &[u8] = b"`\x80`@R4\x80\x15`\x0FW`\0\x80\xFD[Pa\x06\xC4\x80a\0\x1F`\09`\0\xF3\xFE`\x80`@R`\x046\x10a\0?W`\x005`\xE0\x1C\x80c_\xE0?E\x14a\0DW\x80co\x19M\xC9\x14a\0fW\x80c\xA5\xD0\xBB\x93\x14a\0\xB3W\x80c\xA8\xDEm\x8C\x14a\0\xD6W[`\0\x80\xFD[4\x80\x15a\0PW`\0\x80\xFD[Pa\0da\0_6`\x04a\x04\x8CV[a\0\xFDV[\0[4\x80\x15a\0rW`\0\x80\xFD[Pa\0\x99a\0\x816`\x04a\x05\x15V[`\0` \x81\x90R\x90\x81R`@\x90 Tc\xFF\xFF\xFF\xFF\x16\x81V[`@Qc\xFF\xFF\xFF\xFF\x90\x91\x16\x81R` \x01[`@Q\x80\x91\x03\x90\xF3[a\0\xC6a\0\xC16`\x04a\x057V[a\x03LV[`@Q\x90\x15\x15\x81R` \x01a\0\xAAV[4\x80\x15a\0\xE2W`\0\x80\xFD[Pa\0\xEFd\x02T\x0B\xE4\0\x81V[`@Q\x90\x81R` \x01a\0\xAAV[`\0Z`\x01`\x01`\xA0\x1B\x03\x88\x16`\0\x90\x81R` \x81\x90R`@\x90 T\x90\x91Pc\xFF\xFF\xFF\xFF\x90\x81\x16\x90\x86\x16\x11a\x01\x8BW`@QbF\x1B\xCD`\xE5\x1B\x81R` `\x04\x82\x01R`)`$\x82\x01R\x7Fuser bitcoinBlockHeight needs to`D\x82\x01Rh increase`\xB8\x1B`d\x82\x01R`\x84\x01[`@Q\x80\x91\x03\x90\xFD[`\x01`\x01`\xA0\x1B\x03\x87\x16`\0\x81\x81R` \x81\x90R`@\x90\x81\x90 \x80Tc\xFF\xFF\xFF\xFF\x19\x16c\xFF\xFF\xFF\xFF\x89\x16\x17\x90UQ\x7F\x92#D\xDC\x04d\x8C\x0C\xE0(\xEC\xDF\x9B,\x9E\xED\x9Ag\x94\xDB\xB4{w{T\xB0\xCF\xE0i\xF1(\xAA\x90a\x01\xEC\x90\x89\x90\x89\x90\x89\x90\x89\x90a\x05\xCCV[`@Q\x80\x91\x03\x90\xA2`\0:a\x04m`\x03a\x07\xD3aR\x08\x80Za\x02\x0E\x90\x89a\x06\x12V[a\x02\x18\x91\x90a\x06+V[a\x02\"\x91\x90a\x06+V[a\x02,\x91\x90a\x06+V[a\x026\x91\x90a\x06+V[a\x02@\x91\x90a\x06+V[a\x02J\x91\x90a\x06>V[\x90P\x86\x81\x11\x15a\x02\x9CW`@QbF\x1B\xCD`\xE5\x1B\x81R` `\x04\x82\x01R`\x1C`$\x82\x01R\x7FTx cost exceeds pegin amount\0\0\0\0`D\x82\x01R`d\x01a\x01\x82V[a\x02\xA6\x81\x88a\x06\x12V[`@Q\x90\x97P`\x01`\x01`\xA0\x1B\x03\x89\x16\x90\x88\x15a\x08\xFC\x02\x90\x89\x90`\0\x81\x81\x81\x85\x88\x88\xF1\x93PPPP\x15\x80\x15a\x02\xDFW=`\0\x80>=`\0\xFD[P`@Q`\x01`\x01`\xA0\x1B\x03\x84\x16\x90\x82\x15a\x08\xFC\x02\x90\x83\x90`\0\x81\x81\x81\x85\x88\x88\xF1\x93PPPP\x15\x80\x15a\x03\x16W=`\0\x80>=`\0\xFD[P`@Q\x87\x90\x7F\x8E7\xEB.\xE3\xA6\xF3\xC8\xB1;\x89sX\x8D\xAA\xD7ZL\xE7R\xDE\x14\xC0\0\x06\xBD\x82G\xF4\xE2\x12\xE8\x90`\0\x90\xA2PPPPPPPPV[`\0a\x03_d\x02T\x0B\xE4\0a\x01Ja\x06>V[4\x11a\x03\xD3W`@QbF\x1B\xCD`\xE5\x1B\x81R` `\x04\x82\x01R`8`$\x82\x01R\x7FValue must be greater than dust `D\x82\x01R\x7Famount of 330 sats/vByte\0\0\0\0\0\0\0\0`d\x82\x01R`\x84\x01a\x01\x82V[3`\x01`\x01`\xA0\x1B\x03\x16\x7F\x17\xF8y\x87\xDA\x8C\xA7\x1Ciw\x91\xDC\xFD\x19\r\x07c\x0C\xF1{\xF0\x9Ce\xC5\xA5\x9B\x82w\xD9\xFE\x17\x154\x87\x87\x87\x87`@Qa\x04\x14\x95\x94\x93\x92\x91\x90a\x06UV[`@Q\x80\x91\x03\x90\xA2P`\x01\x94\x93PPPPV[\x805`\x01`\x01`\xA0\x1B\x03\x81\x16\x81\x14a\x04>W`\0\x80\xFD[\x91\x90PV[`\0\x80\x83`\x1F\x84\x01\x12a\x04UW`\0\x80\xFD[P\x815g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x04mW`\0\x80\xFD[` \x83\x01\x91P\x83` \x82\x85\x01\x01\x11\x15a\x04\x85W`\0\x80\xFD[\x92P\x92\x90PV[`\0\x80`\0\x80`\0\x80`\xA0\x87\x89\x03\x12\x15a\x04\xA5W`\0\x80\xFD[a\x04\xAE\x87a\x04'V[\x95P` \x87\x015\x94P`@\x87\x015c\xFF\xFF\xFF\xFF\x81\x16\x81\x14a\x04\xCEW`\0\x80\xFD[\x93P``\x87\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x04\xEAW`\0\x80\xFD[a\x04\xF6\x89\x82\x8A\x01a\x04CV[\x90\x94P\x92Pa\x05\t\x90P`\x80\x88\x01a\x04'V[\x90P\x92\x95P\x92\x95P\x92\x95V[`\0` \x82\x84\x03\x12\x15a\x05'W`\0\x80\xFD[a\x050\x82a\x04'V[\x93\x92PPPV[`\0\x80`\0\x80`@\x85\x87\x03\x12\x15a\x05MW`\0\x80\xFD[\x845g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x80\x82\x11\x15a\x05eW`\0\x80\xFD[a\x05q\x88\x83\x89\x01a\x04CV[\x90\x96P\x94P` \x87\x015\x91P\x80\x82\x11\x15a\x05\x8AW`\0\x80\xFD[Pa\x05\x97\x87\x82\x88\x01a\x04CV[\x95\x98\x94\x97P\x95PPPPV[\x81\x83R\x81\x81` \x85\x017P`\0\x82\x82\x01` \x90\x81\x01\x91\x90\x91R`\x1F\x90\x91\x01`\x1F\x19\x16\x90\x91\x01\x01\x90V[\x84\x81Rc\xFF\xFF\xFF\xFF\x84\x16` \x82\x01R```@\x82\x01R`\0a\x05\xF2``\x83\x01\x84\x86a\x05\xA3V[\x96\x95PPPPPPV[cNH{q`\xE0\x1B`\0R`\x11`\x04R`$`\0\xFD[\x81\x81\x03\x81\x81\x11\x15a\x06%Wa\x06%a\x05\xFCV[\x92\x91PPV[\x80\x82\x01\x80\x82\x11\x15a\x06%Wa\x06%a\x05\xFCV[\x80\x82\x02\x81\x15\x82\x82\x04\x84\x14\x17a\x06%Wa\x06%a\x05\xFCV[\x85\x81R``` \x82\x01R`\0a\x06o``\x83\x01\x86\x88a\x05\xA3V[\x82\x81\x03`@\x84\x01Ra\x06\x82\x81\x85\x87a\x05\xA3V[\x98\x97PPPPPPPPV\xFE\xA2dipfsX\"\x12 \xA4h\xD2#\x1B\xFC\x16t]\xB8\xD3\xF0m%\xC3\x94\xDA\x96^\x87rh\xF8\xE3+\x0B\x1F\x9B\x7Fm\x1A\xB0dsolcC\0\x08\x19\x003";
    /// The bytecode of the contract.
    pub static MINTING_BYTECODE: ::ethers::core::types::Bytes =
        ::ethers::core::types::Bytes::from_static(__BYTECODE);
    #[rustfmt::skip]
    const __DEPLOYED_BYTECODE: &[u8] = b"`\x80`@R`\x046\x10a\0?W`\x005`\xE0\x1C\x80c_\xE0?E\x14a\0DW\x80co\x19M\xC9\x14a\0fW\x80c\xA5\xD0\xBB\x93\x14a\0\xB3W\x80c\xA8\xDEm\x8C\x14a\0\xD6W[`\0\x80\xFD[4\x80\x15a\0PW`\0\x80\xFD[Pa\0da\0_6`\x04a\x04\x8CV[a\0\xFDV[\0[4\x80\x15a\0rW`\0\x80\xFD[Pa\0\x99a\0\x816`\x04a\x05\x15V[`\0` \x81\x90R\x90\x81R`@\x90 Tc\xFF\xFF\xFF\xFF\x16\x81V[`@Qc\xFF\xFF\xFF\xFF\x90\x91\x16\x81R` \x01[`@Q\x80\x91\x03\x90\xF3[a\0\xC6a\0\xC16`\x04a\x057V[a\x03LV[`@Q\x90\x15\x15\x81R` \x01a\0\xAAV[4\x80\x15a\0\xE2W`\0\x80\xFD[Pa\0\xEFd\x02T\x0B\xE4\0\x81V[`@Q\x90\x81R` \x01a\0\xAAV[`\0Z`\x01`\x01`\xA0\x1B\x03\x88\x16`\0\x90\x81R` \x81\x90R`@\x90 T\x90\x91Pc\xFF\xFF\xFF\xFF\x90\x81\x16\x90\x86\x16\x11a\x01\x8BW`@QbF\x1B\xCD`\xE5\x1B\x81R` `\x04\x82\x01R`)`$\x82\x01R\x7Fuser bitcoinBlockHeight needs to`D\x82\x01Rh increase`\xB8\x1B`d\x82\x01R`\x84\x01[`@Q\x80\x91\x03\x90\xFD[`\x01`\x01`\xA0\x1B\x03\x87\x16`\0\x81\x81R` \x81\x90R`@\x90\x81\x90 \x80Tc\xFF\xFF\xFF\xFF\x19\x16c\xFF\xFF\xFF\xFF\x89\x16\x17\x90UQ\x7F\x92#D\xDC\x04d\x8C\x0C\xE0(\xEC\xDF\x9B,\x9E\xED\x9Ag\x94\xDB\xB4{w{T\xB0\xCF\xE0i\xF1(\xAA\x90a\x01\xEC\x90\x89\x90\x89\x90\x89\x90\x89\x90a\x05\xCCV[`@Q\x80\x91\x03\x90\xA2`\0:a\x04m`\x03a\x07\xD3aR\x08\x80Za\x02\x0E\x90\x89a\x06\x12V[a\x02\x18\x91\x90a\x06+V[a\x02\"\x91\x90a\x06+V[a\x02,\x91\x90a\x06+V[a\x026\x91\x90a\x06+V[a\x02@\x91\x90a\x06+V[a\x02J\x91\x90a\x06>V[\x90P\x86\x81\x11\x15a\x02\x9CW`@QbF\x1B\xCD`\xE5\x1B\x81R` `\x04\x82\x01R`\x1C`$\x82\x01R\x7FTx cost exceeds pegin amount\0\0\0\0`D\x82\x01R`d\x01a\x01\x82V[a\x02\xA6\x81\x88a\x06\x12V[`@Q\x90\x97P`\x01`\x01`\xA0\x1B\x03\x89\x16\x90\x88\x15a\x08\xFC\x02\x90\x89\x90`\0\x81\x81\x81\x85\x88\x88\xF1\x93PPPP\x15\x80\x15a\x02\xDFW=`\0\x80>=`\0\xFD[P`@Q`\x01`\x01`\xA0\x1B\x03\x84\x16\x90\x82\x15a\x08\xFC\x02\x90\x83\x90`\0\x81\x81\x81\x85\x88\x88\xF1\x93PPPP\x15\x80\x15a\x03\x16W=`\0\x80>=`\0\xFD[P`@Q\x87\x90\x7F\x8E7\xEB.\xE3\xA6\xF3\xC8\xB1;\x89sX\x8D\xAA\xD7ZL\xE7R\xDE\x14\xC0\0\x06\xBD\x82G\xF4\xE2\x12\xE8\x90`\0\x90\xA2PPPPPPPPV[`\0a\x03_d\x02T\x0B\xE4\0a\x01Ja\x06>V[4\x11a\x03\xD3W`@QbF\x1B\xCD`\xE5\x1B\x81R` `\x04\x82\x01R`8`$\x82\x01R\x7FValue must be greater than dust `D\x82\x01R\x7Famount of 330 sats/vByte\0\0\0\0\0\0\0\0`d\x82\x01R`\x84\x01a\x01\x82V[3`\x01`\x01`\xA0\x1B\x03\x16\x7F\x17\xF8y\x87\xDA\x8C\xA7\x1Ciw\x91\xDC\xFD\x19\r\x07c\x0C\xF1{\xF0\x9Ce\xC5\xA5\x9B\x82w\xD9\xFE\x17\x154\x87\x87\x87\x87`@Qa\x04\x14\x95\x94\x93\x92\x91\x90a\x06UV[`@Q\x80\x91\x03\x90\xA2P`\x01\x94\x93PPPPV[\x805`\x01`\x01`\xA0\x1B\x03\x81\x16\x81\x14a\x04>W`\0\x80\xFD[\x91\x90PV[`\0\x80\x83`\x1F\x84\x01\x12a\x04UW`\0\x80\xFD[P\x815g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x04mW`\0\x80\xFD[` \x83\x01\x91P\x83` \x82\x85\x01\x01\x11\x15a\x04\x85W`\0\x80\xFD[\x92P\x92\x90PV[`\0\x80`\0\x80`\0\x80`\xA0\x87\x89\x03\x12\x15a\x04\xA5W`\0\x80\xFD[a\x04\xAE\x87a\x04'V[\x95P` \x87\x015\x94P`@\x87\x015c\xFF\xFF\xFF\xFF\x81\x16\x81\x14a\x04\xCEW`\0\x80\xFD[\x93P``\x87\x015g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x81\x11\x15a\x04\xEAW`\0\x80\xFD[a\x04\xF6\x89\x82\x8A\x01a\x04CV[\x90\x94P\x92Pa\x05\t\x90P`\x80\x88\x01a\x04'V[\x90P\x92\x95P\x92\x95P\x92\x95V[`\0` \x82\x84\x03\x12\x15a\x05'W`\0\x80\xFD[a\x050\x82a\x04'V[\x93\x92PPPV[`\0\x80`\0\x80`@\x85\x87\x03\x12\x15a\x05MW`\0\x80\xFD[\x845g\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFF\x80\x82\x11\x15a\x05eW`\0\x80\xFD[a\x05q\x88\x83\x89\x01a\x04CV[\x90\x96P\x94P` \x87\x015\x91P\x80\x82\x11\x15a\x05\x8AW`\0\x80\xFD[Pa\x05\x97\x87\x82\x88\x01a\x04CV[\x95\x98\x94\x97P\x95PPPPV[\x81\x83R\x81\x81` \x85\x017P`\0\x82\x82\x01` \x90\x81\x01\x91\x90\x91R`\x1F\x90\x91\x01`\x1F\x19\x16\x90\x91\x01\x01\x90V[\x84\x81Rc\xFF\xFF\xFF\xFF\x84\x16` \x82\x01R```@\x82\x01R`\0a\x05\xF2``\x83\x01\x84\x86a\x05\xA3V[\x96\x95PPPPPPV[cNH{q`\xE0\x1B`\0R`\x11`\x04R`$`\0\xFD[\x81\x81\x03\x81\x81\x11\x15a\x06%Wa\x06%a\x05\xFCV[\x92\x91PPV[\x80\x82\x01\x80\x82\x11\x15a\x06%Wa\x06%a\x05\xFCV[\x80\x82\x02\x81\x15\x82\x82\x04\x84\x14\x17a\x06%Wa\x06%a\x05\xFCV[\x85\x81R``` \x82\x01R`\0a\x06o``\x83\x01\x86\x88a\x05\xA3V[\x82\x81\x03`@\x84\x01Ra\x06\x82\x81\x85\x87a\x05\xA3V[\x98\x97PPPPPPPPV\xFE\xA2dipfsX\"\x12 \xA4h\xD2#\x1B\xFC\x16t]\xB8\xD3\xF0m%\xC3\x94\xDA\x96^\x87rh\xF8\xE3+\x0B\x1F\x9B\x7Fm\x1A\xB0dsolcC\0\x08\x19\x003";
    /// The deployed bytecode of the contract.
    pub static MINTING_DEPLOYED_BYTECODE: ::ethers::core::types::Bytes =
        ::ethers::core::types::Bytes::from_static(__DEPLOYED_BYTECODE);
    pub struct Minting<M>(::ethers::contract::Contract<M>);
    impl<M> ::core::clone::Clone for Minting<M> {
        fn clone(&self) -> Self {
            Self(::core::clone::Clone::clone(&self.0))
        }
    }
    impl<M> ::core::ops::Deref for Minting<M> {
        type Target = ::ethers::contract::Contract<M>;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl<M> ::core::ops::DerefMut for Minting<M> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }
    impl<M> ::core::fmt::Debug for Minting<M> {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            f.debug_tuple(::core::stringify!(Minting)).field(&self.address()).finish()
        }
    }
    impl<M: ::ethers::providers::Middleware> Minting<M> {
        /// Creates a new contract instance with the specified `ethers` client at
        /// `address`. The contract derefs to a `ethers::Contract` object.
        pub fn new<T: Into<::ethers::core::types::Address>>(
            address: T,
            client: ::std::sync::Arc<M>,
        ) -> Self {
            Self(::ethers::contract::Contract::new(address.into(), MINTING_ABI.clone(), client))
        }
        /// Constructs the general purpose `Deployer` instance based on the provided constructor
        /// arguments and sends it. Returns a new instance of a deployer that returns an
        /// instance of this contract after sending the transaction
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
                MINTING_ABI.clone(),
                MINTING_BYTECODE.clone().into(),
                client,
            );
            let deployer = factory.deploy(constructor_args)?;
            let deployer = ::ethers::contract::ContractDeployer::new(deployer);
            Ok(deployer)
        }
        ///Calls the contract's `SATS_TO_WEI` (0xa8de6d8c) function
        pub fn sats_to_wei(
            &self,
        ) -> ::ethers::contract::builders::ContractCall<M, ::ethers::core::types::U256> {
            self.0
                .method_hash([168, 222, 109, 140], ())
                .expect("method not found (this should never happen)")
        }
        ///Calls the contract's `burn` (0xa5d0bb93) function
        pub fn burn(
            &self,
            destination: ::ethers::core::types::Bytes,
            data: ::ethers::core::types::Bytes,
        ) -> ::ethers::contract::builders::ContractCall<M, bool> {
            self.0
                .method_hash([165, 208, 187, 147], (destination, data))
                .expect("method not found (this should never happen)")
        }
        ///Calls the contract's `mint` (0x5fe03f45) function
        pub fn mint(
            &self,
            destination: ::ethers::core::types::Address,
            amount: ::ethers::core::types::U256,
            bitcoin_block_height: u32,
            metadata: ::ethers::core::types::Bytes,
            refund_address: ::ethers::core::types::Address,
        ) -> ::ethers::contract::builders::ContractCall<M, ()> {
            self.0
                .method_hash(
                    [95, 224, 63, 69],
                    (destination, amount, bitcoin_block_height, metadata, refund_address),
                )
                .expect("method not found (this should never happen)")
        }
        ///Calls the contract's `peginBitcoinBlockHeight` (0x6f194dc9) function
        pub fn pegin_bitcoin_block_height(
            &self,
            p0: ::ethers::core::types::Address,
        ) -> ::ethers::contract::builders::ContractCall<M, u32> {
            self.0
                .method_hash([111, 25, 77, 201], p0)
                .expect("method not found (this should never happen)")
        }
        ///Gets the contract's `Burn` event
        pub fn burn_filter(
            &self,
        ) -> ::ethers::contract::builders::Event<::std::sync::Arc<M>, M, BurnFilter> {
            self.0.event()
        }
        ///Gets the contract's `Mint` event
        pub fn mint_filter(
            &self,
        ) -> ::ethers::contract::builders::Event<::std::sync::Arc<M>, M, MintFilter> {
            self.0.event()
        }
        ///Gets the contract's `MintAmount` event
        pub fn mint_amount_filter(
            &self,
        ) -> ::ethers::contract::builders::Event<::std::sync::Arc<M>, M, MintAmountFilter> {
            self.0.event()
        }
        /// Returns an `Event` builder for all the events of this contract.
        pub fn events(
            &self,
        ) -> ::ethers::contract::builders::Event<::std::sync::Arc<M>, M, MintingEvents> {
            self.0.event_with_filter(::core::default::Default::default())
        }
    }
    impl<M: ::ethers::providers::Middleware> From<::ethers::contract::Contract<M>> for Minting<M> {
        fn from(contract: ::ethers::contract::Contract<M>) -> Self {
            Self::new(contract.address(), contract.client())
        }
    }
    #[derive(
        Clone,
        ::ethers::contract::EthEvent,
        ::ethers::contract::EthDisplay,
        serde::Serialize,
        serde::Deserialize,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    #[ethevent(name = "Burn", abi = "Burn(address,uint256,bytes,bytes)")]
    pub struct BurnFilter {
        #[ethevent(indexed)]
        pub account: ::ethers::core::types::Address,
        pub amount: ::ethers::core::types::U256,
        pub destination: ::ethers::core::types::Bytes,
        pub metadata: ::ethers::core::types::Bytes,
    }
    #[derive(
        Clone,
        ::ethers::contract::EthEvent,
        ::ethers::contract::EthDisplay,
        serde::Serialize,
        serde::Deserialize,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    #[ethevent(name = "Mint", abi = "Mint(address,uint256,uint32,bytes)")]
    pub struct MintFilter {
        #[ethevent(indexed)]
        pub account: ::ethers::core::types::Address,
        pub amount: ::ethers::core::types::U256,
        pub bitcoin_block_height: u32,
        pub metadata: ::ethers::core::types::Bytes,
    }
    #[derive(
        Clone,
        ::ethers::contract::EthEvent,
        ::ethers::contract::EthDisplay,
        serde::Serialize,
        serde::Deserialize,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    #[ethevent(name = "MintAmount", abi = "MintAmount(uint256)")]
    pub struct MintAmountFilter {
        #[ethevent(indexed)]
        pub amount: ::ethers::core::types::U256,
    }
    ///Container type for all of the contract's events
    #[derive(
        Clone,
        ::ethers::contract::EthAbiType,
        serde::Serialize,
        serde::Deserialize,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    pub enum MintingEvents {
        BurnFilter(BurnFilter),
        MintFilter(MintFilter),
        MintAmountFilter(MintAmountFilter),
    }
    impl ::ethers::contract::EthLogDecode for MintingEvents {
        fn decode_log(
            log: &::ethers::core::abi::RawLog,
        ) -> ::core::result::Result<Self, ::ethers::core::abi::Error> {
            if let Ok(decoded) = BurnFilter::decode_log(log) {
                return Ok(MintingEvents::BurnFilter(decoded));
            }
            if let Ok(decoded) = MintFilter::decode_log(log) {
                return Ok(MintingEvents::MintFilter(decoded));
            }
            if let Ok(decoded) = MintAmountFilter::decode_log(log) {
                return Ok(MintingEvents::MintAmountFilter(decoded));
            }
            Err(::ethers::core::abi::Error::InvalidData)
        }
    }
    impl ::core::fmt::Display for MintingEvents {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            match self {
                Self::BurnFilter(element) => ::core::fmt::Display::fmt(element, f),
                Self::MintFilter(element) => ::core::fmt::Display::fmt(element, f),
                Self::MintAmountFilter(element) => ::core::fmt::Display::fmt(element, f),
            }
        }
    }
    impl ::core::convert::From<BurnFilter> for MintingEvents {
        fn from(value: BurnFilter) -> Self {
            Self::BurnFilter(value)
        }
    }
    impl ::core::convert::From<MintFilter> for MintingEvents {
        fn from(value: MintFilter) -> Self {
            Self::MintFilter(value)
        }
    }
    impl ::core::convert::From<MintAmountFilter> for MintingEvents {
        fn from(value: MintAmountFilter) -> Self {
            Self::MintAmountFilter(value)
        }
    }
    ///Container type for all input parameters for the `SATS_TO_WEI` function with signature
    /// `SATS_TO_WEI()` and selector `0xa8de6d8c`
    #[derive(
        Clone,
        ::ethers::contract::EthCall,
        ::ethers::contract::EthDisplay,
        serde::Serialize,
        serde::Deserialize,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    #[ethcall(name = "SATS_TO_WEI", abi = "SATS_TO_WEI()")]
    pub struct SatsToWeiCall;
    ///Container type for all input parameters for the `burn` function with signature
    /// `burn(bytes,bytes)` and selector `0xa5d0bb93`
    #[derive(
        Clone,
        ::ethers::contract::EthCall,
        ::ethers::contract::EthDisplay,
        serde::Serialize,
        serde::Deserialize,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    #[ethcall(name = "burn", abi = "burn(bytes,bytes)")]
    pub struct BurnCall {
        pub destination: ::ethers::core::types::Bytes,
        pub data: ::ethers::core::types::Bytes,
    }
    ///Container type for all input parameters for the `mint` function with signature
    /// `mint(address,uint256,uint32,bytes,address)` and selector `0x5fe03f45`
    #[derive(
        Clone,
        ::ethers::contract::EthCall,
        ::ethers::contract::EthDisplay,
        serde::Serialize,
        serde::Deserialize,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    #[ethcall(name = "mint", abi = "mint(address,uint256,uint32,bytes,address)")]
    pub struct MintCall {
        pub destination: ::ethers::core::types::Address,
        pub amount: ::ethers::core::types::U256,
        pub bitcoin_block_height: u32,
        pub metadata: ::ethers::core::types::Bytes,
        pub refund_address: ::ethers::core::types::Address,
    }
    ///Container type for all input parameters for the `peginBitcoinBlockHeight` function with
    /// signature `peginBitcoinBlockHeight(address)` and selector `0x6f194dc9`
    #[derive(
        Clone,
        ::ethers::contract::EthCall,
        ::ethers::contract::EthDisplay,
        serde::Serialize,
        serde::Deserialize,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    #[ethcall(name = "peginBitcoinBlockHeight", abi = "peginBitcoinBlockHeight(address)")]
    pub struct PeginBitcoinBlockHeightCall(pub ::ethers::core::types::Address);
    ///Container type for all of the contract's call
    #[derive(
        Clone,
        ::ethers::contract::EthAbiType,
        serde::Serialize,
        serde::Deserialize,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    pub enum MintingCalls {
        SatsToWei(SatsToWeiCall),
        Burn(BurnCall),
        Mint(MintCall),
        PeginBitcoinBlockHeight(PeginBitcoinBlockHeightCall),
    }
    impl ::ethers::core::abi::AbiDecode for MintingCalls {
        fn decode(
            data: impl AsRef<[u8]>,
        ) -> ::core::result::Result<Self, ::ethers::core::abi::AbiError> {
            let data = data.as_ref();
            if let Ok(decoded) = <SatsToWeiCall as ::ethers::core::abi::AbiDecode>::decode(data) {
                return Ok(Self::SatsToWei(decoded));
            }
            if let Ok(decoded) = <BurnCall as ::ethers::core::abi::AbiDecode>::decode(data) {
                return Ok(Self::Burn(decoded));
            }
            if let Ok(decoded) = <MintCall as ::ethers::core::abi::AbiDecode>::decode(data) {
                return Ok(Self::Mint(decoded));
            }
            if let Ok(decoded) =
                <PeginBitcoinBlockHeightCall as ::ethers::core::abi::AbiDecode>::decode(data)
            {
                return Ok(Self::PeginBitcoinBlockHeight(decoded));
            }
            Err(::ethers::core::abi::Error::InvalidData.into())
        }
    }
    impl ::ethers::core::abi::AbiEncode for MintingCalls {
        fn encode(self) -> Vec<u8> {
            match self {
                Self::SatsToWei(element) => ::ethers::core::abi::AbiEncode::encode(element),
                Self::Burn(element) => ::ethers::core::abi::AbiEncode::encode(element),
                Self::Mint(element) => ::ethers::core::abi::AbiEncode::encode(element),
                Self::PeginBitcoinBlockHeight(element) => {
                    ::ethers::core::abi::AbiEncode::encode(element)
                }
            }
        }
    }
    impl ::core::fmt::Display for MintingCalls {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
            match self {
                Self::SatsToWei(element) => ::core::fmt::Display::fmt(element, f),
                Self::Burn(element) => ::core::fmt::Display::fmt(element, f),
                Self::Mint(element) => ::core::fmt::Display::fmt(element, f),
                Self::PeginBitcoinBlockHeight(element) => ::core::fmt::Display::fmt(element, f),
            }
        }
    }
    impl ::core::convert::From<SatsToWeiCall> for MintingCalls {
        fn from(value: SatsToWeiCall) -> Self {
            Self::SatsToWei(value)
        }
    }
    impl ::core::convert::From<BurnCall> for MintingCalls {
        fn from(value: BurnCall) -> Self {
            Self::Burn(value)
        }
    }
    impl ::core::convert::From<MintCall> for MintingCalls {
        fn from(value: MintCall) -> Self {
            Self::Mint(value)
        }
    }
    impl ::core::convert::From<PeginBitcoinBlockHeightCall> for MintingCalls {
        fn from(value: PeginBitcoinBlockHeightCall) -> Self {
            Self::PeginBitcoinBlockHeight(value)
        }
    }
    ///Container type for all return fields from the `SATS_TO_WEI` function with signature
    /// `SATS_TO_WEI()` and selector `0xa8de6d8c`
    #[derive(
        Clone,
        ::ethers::contract::EthAbiType,
        ::ethers::contract::EthAbiCodec,
        serde::Serialize,
        serde::Deserialize,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    pub struct SatsToWeiReturn(pub ::ethers::core::types::U256);
    ///Container type for all return fields from the `burn` function with signature
    /// `burn(bytes,bytes)` and selector `0xa5d0bb93`
    #[derive(
        Clone,
        ::ethers::contract::EthAbiType,
        ::ethers::contract::EthAbiCodec,
        serde::Serialize,
        serde::Deserialize,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    pub struct BurnReturn {
        pub success: bool,
    }
    ///Container type for all return fields from the `peginBitcoinBlockHeight` function with
    /// signature `peginBitcoinBlockHeight(address)` and selector `0x6f194dc9`
    #[derive(
        Clone,
        ::ethers::contract::EthAbiType,
        ::ethers::contract::EthAbiCodec,
        serde::Serialize,
        serde::Deserialize,
        Default,
        Debug,
        PartialEq,
        Eq,
        Hash,
    )]
    pub struct PeginBitcoinBlockHeightReturn(pub u32);
}
