//! Load Network node implementation using Reth's component-based architecture.
//!
//! Composition mirrors upstream `reth`: custom pool builder, passive consensus
//! (Ultramarine drives block production), standard payload/network and executor
//! builders.
//!
//! The goal is to own every major hook where Load may diverge from Ethereum's
//! Prague/Pectra rules (tx pool blob caps, payload builder, EVM executor) while
//! still leaning on the upstream network/RPC layers until bespoke behaviour is
//! required.

use std::sync::Arc;

use alloy_eips::eip4844::BYTES_PER_BLOB;
use reth::{
    api::{BlockTy, NodeTypes, TxTy},
    providers::EthStorage,
};
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_network::{primitives::BasicNetworkPrimitives, NetworkHandle, NetworkManager, PeersInfo};
use reth_node_api::{FullNodeComponents, PrimitivesTy};
use reth_node_builder::{
    components::{BasicPayloadServiceBuilder, ComponentsBuilder, NetworkBuilder, NodeComponents},
    node::FullNodeTypes as FullNodeTypesAlias,
    rpc::{BasicEngineValidatorBuilder, RpcAddOns},
    BuilderContext, Node, NodeAdapter, NodeComponentsBuilder,
};
use reth_node_ethereum::node::EthereumEthApiBuilder;
use reth_payload_primitives::{PayloadAttributesBuilder, PayloadTypes};
use reth_transaction_pool::{PoolPooledTx, PoolTransaction, TransactionPool};
use tracing::info;

use crate::{
    chainspec::LoadChainSpec,
    consensus::LoadConsensusBuilder,
    engine::{
        payload::{LoadEngineTypes, LoadLocalPayloadAttributesBuilder},
        rpc::LoadEngineApiBuilder,
        validator::LoadEngineValidatorBuilder,
        LoadPayloadServiceBuilder,
    },
    evm::{LoadEvmConfig, LoadExecutorBuilder},
    pool::LoadPoolBuilder,
    primitives::LoadPrimitives,
    rpc::backpressure::LoadRpcBackpressureLayer,
    version::load_client_version_string,
};

const LOAD_POOLED_TX_SOFT_LIMIT_BYTES: usize =
    (crate::chainspec::LOAD_MAX_BLOBS_PER_TX as usize) * BYTES_PER_BLOB;

/// Load Network node type configuration.
///
/// This ties the generic `NodeTypes` trait to Load-owned primitives so every
/// subsystem (pool, engine, EVM) can be swapped out incrementally without
/// touching application code.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct LoadNode;

impl NodeTypes for LoadNode {
    type Primitives = LoadPrimitives;
    type ChainSpec = LoadChainSpec;
    type Storage = EthStorage;
    type Payload = LoadEngineTypes;
}

/// Load-specific network builder.
///
/// This currently mirrors the upstream Ethereum builder but keeps the type
/// surface in-tree so we can later tweak gossip soft limits / additional
/// protocols without touching the node wiring.
#[derive(Debug, Default, Clone, Copy)]
pub struct LoadNetworkBuilder;

impl<Node, Pool> NetworkBuilder<Node, Pool> for LoadNetworkBuilder
where
    Node: FullNodeTypesAlias<Types = LoadNode>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
{
    type Network =
        NetworkHandle<BasicNetworkPrimitives<PrimitivesTy<Node::Types>, PoolPooledTx<Pool>>>;

    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::Network> {
        let mut network_builder = ctx
            .network_config_builder::<BasicNetworkPrimitives<PrimitivesTy<Node::Types>, PoolPooledTx<Pool>>>()?;

        let mut tx_config = ctx.config().network.transactions_manager_config();
        tx_config.transaction_fetcher_config.soft_limit_byte_size_pooled_transactions_response =
            LOAD_POOLED_TX_SOFT_LIMIT_BYTES;
        tx_config
            .transaction_fetcher_config
            .soft_limit_byte_size_pooled_transactions_response_on_pack_request =
            LOAD_POOLED_TX_SOFT_LIMIT_BYTES;

        network_builder = network_builder.transactions_manager_config(tx_config);
        let mut network_config = ctx.build_network_config(network_builder);
        network_config.hello_message.client_version = load_client_version_string().to_string();
        let builder = NetworkManager::builder(network_config).await?;
        let handle = ctx.start_network(builder, pool);
        info!(target: "load_reth::network", enr = %handle.local_enr(), "P2P networking initialized");
        Ok(handle)
    }
}

/// Core node wiring that assembles all Load-specific components.
///
/// This impl connects the Load-specific builders (pool, executor, payload,
/// consensus) with the generic reth node infrastructure. The trait bounds
/// ensure type safety across the entire component graph.
///
/// # Components
///
/// - **Pool**: [`LoadPoolBuilder`] with 32K blob cache for high-throughput DA
/// - **Executor**: [`LoadExecutorBuilder`] producing [`LoadEvmConfig`]
/// - **Payload**: [`LoadPayloadServiceBuilder`] enforcing 1024 blob cap
/// - **Consensus**: [`LoadConsensusBuilder`] in passive mode (Ultramarine drives)
/// - **Network**: Upstream Ethereum p2p (to be customized if needed)
impl<N> Node<N> for LoadNode
where
    N: FullNodeTypesAlias<Types = Self>,
    <N::Types as NodeTypes>::Primitives:
        reth_primitives_traits::NodePrimitives<BlockHeader = alloy_consensus::Header>,
    ComponentsBuilder<
        N,
        LoadPoolBuilder,
        BasicPayloadServiceBuilder<LoadPayloadServiceBuilder>,
        LoadNetworkBuilder,
        LoadExecutorBuilder,
        LoadConsensusBuilder,
    >: NodeComponentsBuilder<N>,
    <ComponentsBuilder<
        N,
        LoadPoolBuilder,
        BasicPayloadServiceBuilder<LoadPayloadServiceBuilder>,
        LoadNetworkBuilder,
        LoadExecutorBuilder,
        LoadConsensusBuilder,
    > as NodeComponentsBuilder<N>>::Components: NodeComponents<N, Evm = LoadEvmConfig>,
    LoadConsensusBuilder: reth_node_builder::components::ConsensusBuilder<N>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        LoadPoolBuilder,
        BasicPayloadServiceBuilder<LoadPayloadServiceBuilder>,
        LoadNetworkBuilder,
        LoadExecutorBuilder,
        LoadConsensusBuilder,
    >;

    type AddOns = RpcAddOns<
        NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
        EthereumEthApiBuilder,
        LoadEngineValidatorBuilder,
        LoadEngineApiBuilder<LoadEngineValidatorBuilder>,
        BasicEngineValidatorBuilder<LoadEngineValidatorBuilder>,
        LoadRpcBackpressureLayer,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        // Each component builder is Load specific where necessary (pool,
        // executor, payload builder).  Network/RPC reuse Ethereum defaults for
        // now but can be replaced once Load needs bespoke gossip.
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(LoadPoolBuilder)
            .executor(LoadExecutorBuilder)
            .payload(BasicPayloadServiceBuilder::new(LoadPayloadServiceBuilder::default()))
            .network(LoadNetworkBuilder)
            .consensus(LoadConsensusBuilder)
    }

    fn add_ons(&self) -> Self::AddOns {
        // Engine validators + RPC API talking to Ultramarine.  We keep the
        // Ethereum ETH API for user-facing RPC compatibility, and layer the
        // Load engine service on top.
        RpcAddOns::new(
            EthereumEthApiBuilder::default(),
            LoadEngineValidatorBuilder::default(),
            LoadEngineApiBuilder::<LoadEngineValidatorBuilder>::default(),
            BasicEngineValidatorBuilder::new(LoadEngineValidatorBuilder::default()),
            LoadRpcBackpressureLayer::from_env(),
        )
    }
}

/// Debug/local mode support for Load nodes.
///
/// Enables local block production without a real consensus layer, useful for
/// integration tests and single-node devnets. The payload attributes builder
/// wraps the upstream Ethereum builder but enforces Load's constant PREVRANDAO.
impl<N> reth_node_builder::DebugNode<N> for LoadNode
where
    N: FullNodeComponents<Types = Self>,
{
    type RpcBlock = alloy_rpc_types::Block;

    /// Converts an RPC block representation to the primitive block type.
    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> BlockTy<Self> {
        rpc_block.into_consensus().convert_transactions()
    }

    /// Returns a payload attributes builder for local/debug block production.
    ///
    /// The returned builder produces attributes with Load's fixed PREVRANDAO
    /// value (0x01), ensuring local blocks match production semantics.
    fn local_payload_attributes_builder(
        chain_spec: &Self::ChainSpec,
    ) -> impl PayloadAttributesBuilder<<Self::Payload as PayloadTypes>::PayloadAttributes> {
        LoadLocalPayloadAttributesBuilder(LocalPayloadAttributesBuilder::new(Arc::new(
            chain_spec.clone(),
        )))
    }
}
