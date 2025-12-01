//! Load Network execution client CLI entry point

use std::sync::Arc;

use clap::Parser;
use load_reth::{chainspec::LoadChainSpecParser, node::LoadNode, LoadChainSpec, LoadEvmConfig};
use reth::CliRunner;
use reth_cli_commands::node::NoArgs;
use reth_cli_util::sigsegv_handler;
use reth_ethereum_cli::Cli;
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_node_builder::NodeHandle;
use tracing::info;

// Allocator configuration
#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

fn main() {
    // Install crash handler for better debugging
    sigsegv_handler::install();

    // Enable backtraces by default
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }

    // Components builder for CLI runner (EVM config + consensus)
    let cli_components_builder = |spec: Arc<LoadChainSpec>| {
        (LoadEvmConfig::new(spec.clone()), Arc::new(EthBeaconConsensus::new(spec)))
    };

    if let Err(err) = Cli::<LoadChainSpecParser, NoArgs>::parse()
        .with_runner_and_components::<LoadNode>(
            CliRunner::try_default_runtime().expect("Failed to create default runtime"),
            cli_components_builder,
            async move |builder, _| {
                info!(target: "load_reth::cli", "🚀 Launching Load Network execution client");
                info!(target: "load_reth::cli", "Version: {}", env!("CARGO_PKG_VERSION"));

                let NodeHandle { node: _node, node_exit_future } =
                    builder.node(LoadNode::default()).launch().await?;

                node_exit_future.await
            },
        )
    {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
