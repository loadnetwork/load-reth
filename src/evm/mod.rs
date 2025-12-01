//! Load-specific EVM configuration wrapper.
//!
//! Load targets the Prague/Pectra feature set at genesis, so we want a seam where
//! we can override upstream Ethereum assumptions (for example blob limits or
//! future DA precompiles) without rewriting the entire executor stack.  This
//! module provides a thin wrapper today, but centralises all EVM-facing knobs
//! so the rest of the node can depend on a Load-owned type.

use std::sync::Arc;

use alloy_consensus::BlockHeader;
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::Bytes;
use reth::{api::NodeTypes, revm::context_interface::block::BlobExcessGasAndPrice};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_evm::{ConfigureEngineEvm, ConfigureEvm, EvmEnvFor, ExecutionCtxFor};
use reth_evm_ethereum::EthEvmConfig as UpstreamEvmConfig;
use reth_node_api::FullNodeTypes;
use reth_node_builder::{components::ExecutorBuilder, BuilderContext, PayloadBuilderConfig};
use reth_primitives_traits::{BlockTy, HeaderTy, SealedBlock, SealedHeader};

use crate::{
    chainspec::{
        LoadChainSpec, LOAD_BLOB_UPDATE_FRACTION, LOAD_MAX_BLOBS_PER_TX, LOAD_MAX_BLOB_COUNT,
        LOAD_TARGET_BLOB_COUNT,
    },
    engine::payload::LoadExecutionData,
    primitives::LoadPrimitives,
};

/// Thin wrapper around the upstream Ethereum EVM config with Load-owned typing.
#[derive(Clone, Debug)]
pub struct LoadEvmConfig {
    inner: UpstreamEvmConfig<LoadChainSpec>,
}

impl LoadEvmConfig {
    /// Creates a new config from the Load chain spec.
    pub fn new(spec: Arc<LoadChainSpec>) -> Self {
        Self { inner: UpstreamEvmConfig::new(spec) }
    }

    /// Returns the inner upstream config.
    pub fn inner(&self) -> &UpstreamEvmConfig<LoadChainSpec> {
        &self.inner
    }

    /// Re-wraps an upstream config. Mostly useful for tests.
    pub fn from_inner(inner: UpstreamEvmConfig<LoadChainSpec>) -> Self {
        Self { inner }
    }

    /// Applies builder-configured extra data to the wrapped assembler.
    pub fn with_extra_data(mut self, extra_data: Bytes) -> Self {
        self.inner = self.inner.with_extra_data(extra_data);
        self
    }

    fn load_blob_params_at(&self, timestamp: u64) -> BlobParams {
        let mut params = self
            .inner
            .chain_spec()
            .blob_params_at_timestamp(timestamp)
            .unwrap_or_else(BlobParams::cancun);
        params.target_blob_count = LOAD_TARGET_BLOB_COUNT;
        params.max_blob_count = LOAD_MAX_BLOB_COUNT;
        params.max_blobs_per_tx = LOAD_MAX_BLOBS_PER_TX;
        params.update_fraction = LOAD_BLOB_UPDATE_FRACTION;
        params
    }

    fn enforce_load_blob_env(&self, timestamp: u64, env: &mut EvmEnvFor<Self>) {
        let params = self.load_blob_params_at(timestamp);

        env.cfg_env.max_blobs_per_tx = Some(params.max_blobs_per_tx);
        env.cfg_env.blob_base_fee_update_fraction = Some(LOAD_BLOB_UPDATE_FRACTION as u64);

        if let Some(blob_env) = env.block_env.blob_excess_gas_and_price.as_mut() {
            let max_blob_gas = params.max_blob_gas_per_block();
            if blob_env.excess_blob_gas > max_blob_gas {
                blob_env.excess_blob_gas = max_blob_gas;
            }
            blob_env.blob_gasprice = params.calc_blob_fee(blob_env.excess_blob_gas);
        } else if self.inner.chain_spec().is_cancun_active_at_timestamp(timestamp) {
            env.block_env.blob_excess_gas_and_price = Some(BlobExcessGasAndPrice {
                excess_blob_gas: 0,
                blob_gasprice: params.calc_blob_fee(0),
            });
        }
    }
}

// Today we simply delegate to the upstream configuration, but this impl is the
// choke point for enforcing Prague/Pectra parameters (e.g. blob gas caps) once
// those rules diverge from mainnet Ethereum.
impl ConfigureEvm for LoadEvmConfig {
    type Primitives = LoadPrimitives;
    type Error = <UpstreamEvmConfig<LoadChainSpec> as ConfigureEvm>::Error;
    type NextBlockEnvCtx = <UpstreamEvmConfig<LoadChainSpec> as ConfigureEvm>::NextBlockEnvCtx;
    type BlockExecutorFactory =
        <UpstreamEvmConfig<LoadChainSpec> as ConfigureEvm>::BlockExecutorFactory;
    type BlockAssembler = <UpstreamEvmConfig<LoadChainSpec> as ConfigureEvm>::BlockAssembler;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self.inner.block_executor_factory()
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.inner.block_assembler()
    }

    fn evm_env(&self, header: &HeaderTy<Self::Primitives>) -> Result<EvmEnvFor<Self>, Self::Error> {
        let mut env = self.inner.evm_env(header)?;
        self.enforce_load_blob_env(header.timestamp(), &mut env);
        Ok(env)
    }

    fn next_evm_env(
        &self,
        parent: &HeaderTy<Self::Primitives>,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        let mut env = self.inner.next_evm_env(parent, attributes)?;
        self.enforce_load_blob_env(attributes.timestamp, &mut env);
        Ok(env)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<BlockTy<Self::Primitives>>,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        self.inner.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<HeaderTy<Self::Primitives>>,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<ExecutionCtxFor<'_, Self>, Self::Error> {
        self.inner.context_for_next_block(parent, attributes)
    }
}

impl ConfigureEngineEvm<LoadExecutionData> for LoadEvmConfig {
    // Cancun terminology is still present in the upstream sidecar types; Load
    // activates Prague immediately, so these delegations ensure we honour the
    // same semantics while retaining the ability to hook in Pectra-specific
    // logic later.
    fn evm_env_for_payload(
        &self,
        payload: &LoadExecutionData,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        let mut env = self.inner.evm_env_for_payload(payload)?;
        self.enforce_load_blob_env(payload.payload().timestamp(), &mut env);
        Ok(env)
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a LoadExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        self.inner.context_for_payload(payload)
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &LoadExecutionData,
    ) -> Result<impl reth_evm::ExecutableTxIterator<Self>, Self::Error> {
        self.inner.tx_iterator_for_payload(payload)
    }
}

/// Builder for the Load EVM executor.
///
/// Integrates with reth's node builder to produce a [`LoadEvmConfig`] that
/// carries the chain spec and any builder-configured extra data (e.g. block
/// producer identity).
#[derive(Debug, Default, Clone, Copy)]
pub struct LoadExecutorBuilder;

impl<Types, Node> ExecutorBuilder<Node> for LoadExecutorBuilder
where
    Types: NodeTypes<ChainSpec = LoadChainSpec, Primitives = LoadPrimitives>,
    Node: FullNodeTypes<Types = Types>,
{
    type EVM = LoadEvmConfig;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        // Builder extra data (e.g. Load branding) needs to flow through the Load
        // wrapper so we do not leak the upstream `EthEvmConfig` type anywhere.
        Ok(LoadEvmConfig::new(ctx.chain_spec())
            .with_extra_data(ctx.payload_builder_config().extra_data_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use alloy_consensus::Header as ConsensusHeader;
    use reth_chainspec::EthereumHardforks;
    use super::*;
    use crate::chainspec::LoadChainSpec;

    #[test]
    fn load_evm_config_creation() {
        let spec = Arc::new(LoadChainSpec::default());
        let config = LoadEvmConfig::new(spec);
        // Verify we can access the inner config and chainspec has expected forks
        assert!(config.inner().chain_spec().is_cancun_active_at_timestamp(0));
    }

    #[test]
    fn load_evm_config_with_extra_data() {
        let spec = Arc::new(LoadChainSpec::default());
        let extra_data = Bytes::from_static(b"Load Network");
        let config = LoadEvmConfig::new(spec).with_extra_data(extra_data);
        // Config should be created without panic
        let _ = config.inner();
    }

    #[test]
    fn load_evm_config_from_inner() {
        let spec = Arc::new(LoadChainSpec::default());
        let inner = UpstreamEvmConfig::new(spec);
        let config = LoadEvmConfig::from_inner(inner);
        assert!(config.inner().chain_spec().is_prague_active_at_timestamp(0));
    }

    #[test]
    fn load_executor_builder_is_default() {
        let builder = LoadExecutorBuilder::default();
        // Just verify it can be created
        let _ = builder;
    }

    #[test]
    fn evm_env_uses_load_blob_params() {
        let spec = Arc::new(LoadChainSpec::default());
        let config = LoadEvmConfig::new(spec);
        let mut header = ConsensusHeader::default();
        header.gas_limit = 30_000_000;
        header.base_fee_per_gas = Some(1);
        header.timestamp = 1;

        let env = config.evm_env(&header).expect("env");

        assert_eq!(env.cfg_env.max_blobs_per_tx, Some(LOAD_MAX_BLOBS_PER_TX));
        assert_eq!(
            env.cfg_env.blob_base_fee_update_fraction,
            Some(LOAD_BLOB_UPDATE_FRACTION as u64)
        );
        assert!(env.block_env.blob_excess_gas_and_price.is_some());
    }

    #[test]
    fn blob_env_is_clamped_to_load_limits() {
        let spec = Arc::new(LoadChainSpec::default());
        let config = LoadEvmConfig::new(spec);
        let params = config.load_blob_params_at(0);
        let max_blob_gas = params.max_blob_gas_per_block();
        let mut env = config.evm_env(&ConsensusHeader::default()).expect("env");
        env.block_env.blob_excess_gas_and_price = Some(BlobExcessGasAndPrice {
            excess_blob_gas: max_blob_gas.saturating_add(1),
            blob_gasprice: 0,
        });

        config.enforce_load_blob_env(0, &mut env);

        let blob_env = env.block_env.blob_excess_gas_and_price.expect("present");
        assert_eq!(env.cfg_env.max_blobs_per_tx, Some(LOAD_MAX_BLOBS_PER_TX));
        assert_eq!(
            env.cfg_env.blob_base_fee_update_fraction,
            Some(LOAD_BLOB_UPDATE_FRACTION as u64)
        );
        assert_eq!(blob_env.excess_blob_gas, max_blob_gas);
    }
}
