//! Passive consensus wiring for the Load Network.
//!
//! We reuse `EthBeaconConsensus` for header/body validation only. Block production/finality is
//! driven externally by Ultramarine via Engine API.

use std::sync::Arc;

use reth::{api::NodeTypes, beacon_consensus::EthBeaconConsensus};
use reth_node_api::FullNodeTypes;
use reth_node_builder::{components::ConsensusBuilder, BuilderContext};

use crate::chainspec::LoadChainSpec;

/// Consensus builder that wraps the standard Ethereum beacon consensus in passive mode.
#[derive(Debug, Default, Clone, Copy)]
pub struct LoadConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for LoadConsensusBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            ChainSpec = LoadChainSpec,
            Primitives: reth_primitives_traits::NodePrimitives<
                BlockHeader = alloy_consensus::Header,
            >,
        >,
    >,
{
    type Consensus = Arc<EthBeaconConsensus<LoadChainSpec>>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(Arc::new(EthBeaconConsensus::new(ctx.chain_spec())))
    }
}
