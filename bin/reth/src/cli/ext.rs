//! Support for integrating customizations into the CLI.

use clap::Args;
use reth_db::DatabaseEnv;
use reth_network::NetworkHandle;
use reth_provider::providers::BlockchainProvider;
use reth_tasks::TaskExecutor;
use std::{fmt, sync::Arc};
use tracing::info;

/// A trait that allows for extending parts of the CLI with additional functionality.
///
/// This is intended as a way to allow to _extend_ the node command. For example, to register
/// additional RPC namespaces.
pub trait RethCliExt {
    /// Provides additional configuration for the node CLI command.
    ///
    /// This supports additional CLI arguments that can be used to modify the node configuration.
    ///
    /// If no additional CLI arguments are required, the [NoArgs] wrapper type can be used.
    type Node: RethNodeCommandExt;
}

/// The default CLI extension.
impl RethCliExt for () {
    type Node = DefaultRethNodeCommandConfig;
}

/// Node components passed to the `on_node_started` hook
pub struct RethNodeComponents {
    /// The task executor
    pub executor: TaskExecutor,
    /// The database
    pub db: BlockchainProvider<Arc<DatabaseEnv>>,
    /// The network handle
    pub network: NetworkHandle,
}

/// A trait that allows for extending and customizing parts of the node command
/// Currently only used by the PoaNodeCommand
/// [on_node_started](PoaNodeCommandConfig::on_node_started)
pub trait PoaNodeCommandConfig: fmt::Debug {
    /// Event hook called once the node has been launched.
    ///
    /// This is called last after the node has been launched.
    fn on_node_started(&self, components: RethNodeComponents) -> eyre::Result<()> {
        info!("on_node_started fired in PoaNodeCommand default impl...");
        let _ = components;
        Ok(())
    }
}

/// A trait that allows for extending parts of the CLI with additional functionality.
pub trait RethNodeCommandExt: PoaNodeCommandConfig + fmt::Debug + clap::Args {}

// blanket impl for all types that implement the required traits.
impl<T> RethNodeCommandExt for T where T: PoaNodeCommandConfig + fmt::Debug + clap::Args {}

/// The default configuration for the reth node command [Command](crate::node::NodeCommand).
///
/// This is a convenience type for [NoArgs<()>].
#[derive(Debug, Clone, Copy, Default, Args)]
#[non_exhaustive]
pub struct DefaultRethNodeCommandConfig;

impl PoaNodeCommandConfig for DefaultRethNodeCommandConfig {}

impl PoaNodeCommandConfig for () {}

// /// A helper type for [RethCliExt] extension that don't require any additional clap Arguments.
// #[derive(Debug, Clone, Copy)]
// pub struct NoArgsCliExt<Conf>(PhantomData<Conf>);

// impl<Conf: PoaNodeCommandConfig> RethCliExt for NoArgsCliExt<Conf> {
//     type Node = NoArgs<Conf>;
// }

/// A helper struct that allows for wrapping a [PoaNodeCommandConfig] value without providing
/// additional CLI arguments.
///
/// Note: This type must be manually filled with a [PoaNodeCommandConfig] manually before executing
/// the [NodeCommand](crate::node::NodeCommand).
#[derive(Debug, Clone, Copy, Default, Args)]
pub struct NoArgs<T = ()> {
    #[clap(skip)]
    inner: Option<T>,
}

impl<T> NoArgs<T> {
    /// Creates a new instance of the wrapper type.
    pub fn with(inner: T) -> Self {
        Self { inner: Some(inner) }
    }

    /// Sets the inner value.
    pub fn set(&mut self, inner: T) {
        self.inner = Some(inner)
    }

    /// Transforms the configured value.
    pub fn map<U>(self, inner: U) -> NoArgs<U> {
        NoArgs::with(inner)
    }

    /// Returns the inner value if it exists.
    pub fn inner(&self) -> Option<&T> {
        self.inner.as_ref()
    }

    /// Returns a mutable reference to the inner value if it exists.
    pub fn inner_mut(&mut self) -> Option<&mut T> {
        self.inner.as_mut()
    }

    /// Consumes the wrapper and returns the inner value if it exists.
    pub fn into_inner(self) -> Option<T> {
        self.inner
    }
}

impl<T: PoaNodeCommandConfig> PoaNodeCommandConfig for NoArgs<T> {
    fn on_node_started(&self, components: RethNodeComponents) -> eyre::Result<()> {
        info!("on_node_started fired in NoArgs default impl...");
        if let Some(conf) = self.inner() {
            conf.on_node_started(components)
        } else {
            Ok(())
        }
    }
}

impl<T> From<T> for NoArgs<T> {
    fn from(value: T) -> Self {
        Self::with(value)
    }
}
