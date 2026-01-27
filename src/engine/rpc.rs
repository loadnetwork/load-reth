//! Load-specific Engine API wiring.
//!
//! Wraps the upstream `EngineApi` to:
//! - reuse Load payload types/builder (already configured in the node),
//! - lift blob request limits to `LOAD_MAX_BLOB_COUNT`,
//! - keep a hook surface for future fork/attribute guards.

use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use alloy_eips::{eip4844::BlobAndProofV2, eip7685::RequestsOrHash};
use alloy_primitives::B256;
use alloy_rpc_types_engine::{
    BlobAndProofV1, ExecutionPayloadBodiesV1, ExecutionPayloadInputV2, ExecutionPayloadV1,
    ExecutionPayloadV3, ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus,
};
use async_trait::async_trait;
use jsonrpsee::{core::RpcResult, RpcModule};
use reth::{
    api::NodeTypes,
    payload::PayloadStore,
    providers::{BlockReader, HeaderProvider, StateProviderFactory},
    rpc::api::IntoEngineApiRpcModule,
};
use reth_chainspec::EthereumHardforks;
use reth_engine_primitives::{EngineApiValidator, EngineTypes};
use reth_network::NetworkInfo;
use reth_node_api::{AddOnsContext, FullNodeComponents};
use reth_node_builder::rpc::PayloadValidatorBuilder;
use reth_node_core::version::{version_metadata, CLIENT_CODE};
use reth_payload_primitives::{EngineObjectValidationError, PayloadTypes};
use reth_rpc_api::EngineApiServer;
use reth_rpc_engine_api::{EngineApi, EngineApiError, EngineCapabilities};
use reth_transaction_pool::TransactionPool;
use tracing::trace;
use std::fmt;

use crate::{
    chainspec::{LoadChainSpec, LOAD_MAX_BLOB_COUNT},
    engine::payload::{LoadExecutionData, LoadPayloadAttributes},
    metrics::LoadEngineRpcMetrics,
    version::{load_client_version_entry, load_client_version_string},
};

const LOAD_CAP_BLOBS: &str = "load.blobs.1024";
const LOAD_CAP_PREVRANDAO: &str = "load.prev_randao.0x01";

/// Engine API builder specialized for Load.
#[derive(Debug, Clone)]
pub struct LoadEngineApiBuilder<PVB> {
    payload_validator_builder: PVB,
}

impl<PVB> LoadEngineApiBuilder<PVB> {
    pub const fn new(payload_validator_builder: PVB) -> Self {
        Self { payload_validator_builder }
    }
}

impl<PVB: Default> Default for LoadEngineApiBuilder<PVB> {
    fn default() -> Self {
        Self { payload_validator_builder: PVB::default() }
    }
}

impl<N, PVB> reth_node_builder::rpc::EngineApiBuilder<N> for LoadEngineApiBuilder<PVB>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec = LoadChainSpec,
            Payload: PayloadTypes<
                ExecutionData = LoadExecutionData,
                PayloadAttributes = crate::engine::payload::LoadPayloadAttributes,
            > + EngineTypes,
        >,
    >,
    PVB: PayloadValidatorBuilder<N>,
    PVB::Validator: EngineApiValidator<<N::Types as NodeTypes>::Payload>,
{
    type EngineApi =
        LoadEngineApi<N::Provider, <N::Types as NodeTypes>::Payload, N::Pool, PVB::Validator>;

    async fn build_engine_api(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::EngineApi> {
        // Build the validator and the underlying Engine API using the provided builders.
        let engine_validator = self.payload_validator_builder.build(ctx).await?;
        let client = alloy_rpc_types_engine::ClientVersionV1 {
            code: CLIENT_CODE,
            name: version_metadata().name_client.to_string(),
            version: version_metadata().cargo_pkg_version.to_string(),
            commit: version_metadata().vergen_git_sha.to_string(),
        };

        let mut capabilities = EngineCapabilities::default();
        capabilities.add_capability(LOAD_CAP_BLOBS);
        capabilities.add_capability(LOAD_CAP_PREVRANDAO);

        let inner = EngineApi::new(
            ctx.node.provider().clone(),
            ctx.config.chain.clone(),
            ctx.beacon_engine_handle.clone(),
            PayloadStore::new(ctx.node.payload_builder_handle().clone()),
            ctx.node.pool().clone(),
            Box::new(ctx.node.task_executor().clone()),
            client,
            capabilities,
            engine_validator,
            ctx.config.engine.accept_execution_requests_hash,
            ctx.node.network().clone(),
        );

        let engine_metrics = Arc::new(LoadEngineRpcMetrics::new());
        let network = ctx.node.network().clone();
        let is_syncing = Arc::new(move || network.is_syncing());

        // Wrap with Load-specific behaviour.
        Ok(LoadEngineApi::new(
            inner,
            ctx.node.pool().clone(),
            engine_metrics,
            is_syncing,
        ))
    }
}

/// Load Engine API wrapper that overrides blob limits.
#[derive(Clone)]
pub struct LoadEngineApi<Provider, PayloadT: PayloadTypes, Pool, Validator> {
    inner: EngineApi<Provider, PayloadT, Pool, Validator, LoadChainSpec>,
    pool: Pool,
    metrics: Arc<LoadEngineRpcMetrics>,
    is_syncing: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl<Provider, PayloadT: PayloadTypes, Pool, Validator> fmt::Debug
    for LoadEngineApi<Provider, PayloadT, Pool, Validator>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadEngineApi").finish_non_exhaustive()
    }
}

impl<Provider, PayloadT: PayloadTypes, Pool, Validator>
    LoadEngineApi<Provider, PayloadT, Pool, Validator>
{
    pub const fn new(
        inner: EngineApi<Provider, PayloadT, Pool, Validator, LoadChainSpec>,
        pool: Pool,
        metrics: Arc<LoadEngineRpcMetrics>,
        is_syncing: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self { inner, pool, metrics, is_syncing }
    }
}

fn validate_blob_request(versioned_hashes: &[B256]) -> Result<(), EngineApiError> {
    if versioned_hashes.len() > LOAD_MAX_BLOB_COUNT as usize {
        return Err(EngineApiError::BlobRequestTooLarge { len: versioned_hashes.len() });
    }
    Ok(())
}

fn ensure_load_prev_randao(prev_randao: &B256) -> Result<(), EngineApiError> {
    if prev_randao.as_slice() == crate::LOAD_PREVRANDAO {
        Ok(())
    } else {
        Err(EngineApiError::EngineObjectValidationError(
            reth_payload_primitives::EngineObjectValidationError::InvalidParams(
                "prev_randao must be constant 0x01 for Load".into(),
            ),
        ))
    }
}

fn payload_v3_prev_randao(payload: &ExecutionPayloadV3) -> B256 {
    payload.payload_inner.payload_inner.prev_randao
}

#[async_trait]
impl<Provider, EngineT, Pool, Validator> EngineApiServer<EngineT>
    for LoadEngineApi<Provider, EngineT, Pool, Validator>
where
    Provider: HeaderProvider + BlockReader + StateProviderFactory + 'static,
    EngineT: EngineTypes<ExecutionData = LoadExecutionData>
        + PayloadTypes<PayloadAttributes = LoadPayloadAttributes>,
    Pool: TransactionPool + Clone + 'static,
    Validator: EngineApiValidator<EngineT>,
{
    async fn new_payload_v1(&self, payload: ExecutionPayloadV1) -> RpcResult<PayloadStatus> {
        let payload = LoadExecutionData::from(payload);
        let start = Instant::now();
        let result = self.inner.new_payload_v1_metered(payload).await?;
        self.metrics.record_new_payload(start.elapsed());
        Ok(result)
    }

    async fn new_payload_v2(&self, payload: ExecutionPayloadInputV2) -> RpcResult<PayloadStatus> {
        let payload = LoadExecutionData::from(payload);
        let start = Instant::now();
        let result = self.inner.new_payload_v2_metered(payload).await?;
        self.metrics.record_new_payload(start.elapsed());
        Ok(result)
    }

    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus> {
        if let Err(err) = validate_blob_request(&versioned_hashes) {
            return Err(err.into());
        }
        let prev_randao = payload_v3_prev_randao(&payload);
        if let Err(err) = ensure_load_prev_randao(&prev_randao) {
            return Err(err.into());
        }
        let payload = LoadExecutionData::v3(payload, versioned_hashes, parent_beacon_block_root);
        let start = Instant::now();
        let result = self.inner.new_payload_v3_metered(payload).await?;
        self.metrics.record_new_payload(start.elapsed());
        Ok(result)
    }

    async fn new_payload_v4(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: RequestsOrHash,
    ) -> RpcResult<PayloadStatus> {
        if let Err(err) = validate_blob_request(&versioned_hashes) {
            return Err(err.into());
        }
        let prev_randao = payload_v3_prev_randao(&payload);
        if let Err(err) = ensure_load_prev_randao(&prev_randao) {
            return Err(err.into());
        }
        let payload = LoadExecutionData::v4(
            payload,
            versioned_hashes,
            parent_beacon_block_root,
            execution_requests,
        );
        let start = Instant::now();
        let result = self.inner.new_payload_v4_metered(payload).await?;
        self.metrics.record_new_payload(start.elapsed());
        Ok(result)
    }

    async fn fork_choice_updated_v1(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<<EngineT as PayloadTypes>::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        let start = Instant::now();
        let result = self
            .inner
            .fork_choice_updated_v1_metered(fork_choice_state, payload_attributes)
            .await?;
        self.metrics.record_forkchoice(start.elapsed());
        Ok(result)
    }

    async fn fork_choice_updated_v2(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<<EngineT as PayloadTypes>::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        let start = Instant::now();
        let result = self
            .inner
            .fork_choice_updated_v2_metered(fork_choice_state, payload_attributes)
            .await?;
        self.metrics.record_forkchoice(start.elapsed());
        Ok(result)
    }

    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<<EngineT as PayloadTypes>::PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        if let Some(attrs) = &payload_attributes {
            let prev_randao = attrs.prev_randao();
            if let Err(err) = ensure_load_prev_randao(&prev_randao) {
                return Err(err.into());
            }
        }

        let start = Instant::now();
        let result = self
            .inner
            .fork_choice_updated_v3_metered(fork_choice_state, payload_attributes)
            .await?;
        self.metrics.record_forkchoice(start.elapsed());
        Ok(result)
    }

    async fn get_payload_v1(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV1> {
        let start = Instant::now();
        let result = self.inner.get_payload_v1_metered(payload_id).await?;
        self.metrics.record_get_payload(start.elapsed());
        Ok(result)
    }

    async fn get_payload_v2(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV2> {
        let start = Instant::now();
        let result = self.inner.get_payload_v2_metered(payload_id).await?;
        self.metrics.record_get_payload(start.elapsed());
        Ok(result)
    }

    async fn get_payload_v3(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV3> {
        let start = Instant::now();
        let result = self.inner.get_payload_v3_metered(payload_id).await?;
        self.metrics.record_get_payload(start.elapsed());
        Ok(result)
    }

    async fn get_payload_v4(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV4> {
        let start = Instant::now();
        let result = self.inner.get_payload_v4_metered(payload_id).await?;
        self.metrics.record_get_payload(start.elapsed());
        Ok(result)
    }

    async fn get_payload_v5(
        &self,
        payload_id: PayloadId,
    ) -> RpcResult<EngineT::ExecutionPayloadEnvelopeV5> {
        let start = Instant::now();
        let result = self.inner.get_payload_v5_metered(payload_id).await?;
        self.metrics.record_get_payload(start.elapsed());
        Ok(result)
    }

    async fn get_payload_bodies_by_hash_v1(
        &self,
        hashes: Vec<B256>,
    ) -> RpcResult<ExecutionPayloadBodiesV1> {
        Ok(self.inner.get_payload_bodies_by_hash_v1_metered(hashes).await?)
    }

    async fn get_payload_bodies_by_range_v1(
        &self,
        start: alloy_primitives::U64,
        count: alloy_primitives::U64,
    ) -> RpcResult<ExecutionPayloadBodiesV1> {
        Ok(self.inner.get_payload_bodies_by_range_v1_metered(start.to(), count.to()).await?)
    }

    async fn get_client_version_v1(
        &self,
        client_version: alloy_rpc_types_engine::ClientVersionV1,
    ) -> RpcResult<Vec<alloy_rpc_types_engine::ClientVersionV1>> {
        let mut versions = self.inner.get_client_version_v1(client_version)?;
        versions.push(load_client_version_entry());
        Ok(versions)
    }

    async fn exchange_capabilities(&self, capabilities: Vec<String>) -> RpcResult<Vec<String>> {
        let mut caps = self.inner.capabilities().clone();
        caps.add_capability(LOAD_CAP_BLOBS);
        caps.add_capability(LOAD_CAP_PREVRANDAO);
        caps.add_capability(load_client_version_string().to_string());
        // Echo peer capabilities plus ours.
        for cap in capabilities {
            caps.add_capability(cap);
        }
        Ok(caps.list())
    }

    async fn get_blobs_v1(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> RpcResult<Vec<Option<BlobAndProofV1>>> {
        trace!(target: "rpc::engine", "Serving engine_getBlobsV1 (Load)");
        if let Err(err) = validate_blob_request(&versioned_hashes) {
            return Err(err.into());
        }

        let blobs = match self.pool.get_blobs_for_versioned_hashes_v1(&versioned_hashes) {
            Ok(blobs) => blobs,
            Err(err) => return Err(EngineApiError::Internal(Box::new(err)).into()),
        };
        let hits = blobs.iter().filter(|entry| entry.is_some()).count() as u64;
        let misses = blobs.len() as u64 - hits;
        self.metrics.record_get_blobs(hits, misses);
        Ok(blobs)
    }

    async fn get_blobs_v2(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> RpcResult<Option<Vec<BlobAndProofV2>>> {
        trace!(target: "rpc::engine", "Serving engine_getBlobsV2 (Load)");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        if !self.inner.chain_spec().is_osaka_active_at_timestamp(now) {
            return Err(EngineApiError::EngineObjectValidationError(
                EngineObjectValidationError::UnsupportedFork,
            )
            .into());
        }
        if let Err(err) = validate_blob_request(&versioned_hashes) {
            return Err(err.into());
        }

        let blobs = match self.pool.get_blobs_for_versioned_hashes_v2(&versioned_hashes) {
            Ok(blobs) => blobs,
            Err(err) => return Err(EngineApiError::Internal(Box::new(err)).into()),
        };
        if let Some(entries) = &blobs {
            let hits = entries.len() as u64;
            let misses = versioned_hashes.len() as u64 - hits;
            self.metrics.record_get_blobs(hits, misses);
        } else {
            self.metrics.record_get_blobs(0, versioned_hashes.len() as u64);
        }
        Ok(blobs)
    }

    async fn get_blobs_v3(
        &self,
        versioned_hashes: Vec<B256>,
    ) -> RpcResult<Option<Vec<Option<BlobAndProofV2>>>> {
        trace!(target: "rpc::engine", "Serving engine_getBlobsV3 (Load)");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        if !self.inner.chain_spec().is_osaka_active_at_timestamp(now) {
            return Err(EngineApiError::EngineObjectValidationError(
                EngineObjectValidationError::UnsupportedFork,
            )
            .into());
        }
        if let Err(err) = validate_blob_request(&versioned_hashes) {
            return Err(err.into());
        }
        if (self.is_syncing)() {
            return Ok(None);
        }

        let blobs = match self.pool.get_blobs_for_versioned_hashes_v3(&versioned_hashes) {
            Ok(blobs) => blobs,
            Err(err) => return Err(EngineApiError::Internal(Box::new(err)).into()),
        };
        let hits = blobs.iter().filter(|entry| entry.is_some()).count() as u64;
        let misses = blobs.len() as u64 - hits;
        self.metrics.record_get_blobs(hits, misses);
        Ok(Some(blobs))
    }
}

impl<Provider, PayloadT, Pool, Validator> IntoEngineApiRpcModule
    for LoadEngineApi<Provider, PayloadT, Pool, Validator>
where
    PayloadT: PayloadTypes + EngineTypes,
    Self: EngineApiServer<PayloadT>,
{
    fn into_rpc_module(self) -> RpcModule<()> {
        self.into_rpc().remove_context()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_limit_respects_load_cap() {
        assert!(validate_blob_request(&vec![B256::ZERO; LOAD_MAX_BLOB_COUNT as usize]).is_ok());
        let err = validate_blob_request(&vec![B256::ZERO; LOAD_MAX_BLOB_COUNT as usize + 1]);
        assert!(matches!(err, Err(EngineApiError::BlobRequestTooLarge { .. })));
    }

    #[test]
    fn prev_randao_guard_accepts_constant() {
        assert!(ensure_load_prev_randao(&B256::from(crate::LOAD_PREVRANDAO)).is_ok());
    }

    #[test]
    fn prev_randao_guard_rejects_other_values() {
        let err = ensure_load_prev_randao(&B256::ZERO).unwrap_err();
        assert!(matches!(err, EngineApiError::EngineObjectValidationError(_)));
    }
}
