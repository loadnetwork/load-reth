//! Transaction pool builder for the Load Network with custom blob cache sizing.
//!
//! Blob cache sizing is derived from the chain spec (target blob count) unless
//! overridden via CLI/config. With Load target=512, the auto size yields
//! `512 * 32 * 2 = 32_768` blobs (~4.3 GB).

use std::time::{Duration, SystemTime};

use alloy_eips::{eip7840::BlobParams, merge::EPOCH_SLOTS};
use reth::{api::NodeTypes, providers::CanonStateSubscriptions};
use reth_chainspec::EthChainSpec;
use reth_ethereum_primitives::TransactionSigned;
use reth_node_api::FullNodeTypes;
use reth_node_builder::{
    components::{PoolBuilder, TxPoolBuilder},
    BuilderContext,
};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, BlobStore, EthTransactionPool, SubPoolLimit,
    TransactionValidationTaskExecutor,
};
use tokio::time::interval;
use tracing::{debug, info};

use crate::{
    chainspec::{LoadChainSpec, LOAD_MAX_BLOBS_PER_TX, LOAD_TARGET_BLOB_COUNT},
    metrics::LoadBlobCacheMetrics,
};

/// Load Network transaction pool builder.
#[derive(Debug, Default, Clone, Copy)]
pub struct LoadPoolBuilder;

impl<Types, Node> PoolBuilder<Node> for LoadPoolBuilder
where
    Types: NodeTypes<
        ChainSpec = LoadChainSpec,
        Primitives: reth_primitives_traits::NodePrimitives<SignedTx = TransactionSigned>,
    >,
    Node: FullNodeTypes<Types = Types>,
    Node::Provider: CanonStateSubscriptions,
{
    type Pool = EthTransactionPool<Node::Provider, DiskFileBlobStore>;

    async fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        let mut pool_config = ctx.pool_config();

        let current_timestamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        let blob_params = ctx
            .chain_spec()
            .blob_params_at_timestamp(current_timestamp)
            .unwrap_or_else(BlobParams::cancun);

        // Derive blob cache size from the target blob count (2 epochs worth) unless overridden.
        let calculated_blob_cache =
            (blob_params.target_blob_count.saturating_mul(EPOCH_SLOTS).saturating_mul(2)) as u32;
        let blob_cache_size = pool_config.blob_cache_size.unwrap_or(calculated_blob_cache);
        pool_config.blob_cache_size = Some(blob_cache_size);

        info!(
            target: "load_reth::pool",
            blob_cache_size,
            target_blob_count = blob_params.target_blob_count,
            "Blob cache size configured"
        );

        // Scale blob subpool limits to Load throughput. We size by the blob cache and allow
        // multiple epochs of backlog, capped per-tx by LOAD_MAX_BLOBS_PER_TX.
        let min_blob_txs = (blob_cache_size as usize)
            .saturating_mul(4)
            .checked_div(LOAD_MAX_BLOBS_PER_TX as usize)
            .unwrap_or(usize::MAX)
            .max(LOAD_TARGET_BLOB_COUNT as usize);

        let default_blob_limit = pool_config.blob_limit;
        let target_blob_txs = default_blob_limit.max_txs.max(min_blob_txs);
        let scale =
            (target_blob_txs + default_blob_limit.max_txs - 1) / default_blob_limit.max_txs.max(1);

        pool_config.blob_limit =
            SubPoolLimit::new(target_blob_txs, default_blob_limit.max_size.saturating_mul(scale));

        info!(
            target: "load_reth::pool",
            max_blob_txs = pool_config.blob_limit.max_txs,
            max_blob_size_bytes = pool_config.blob_limit.max_size,
            default_blob_limit = default_blob_limit.max_txs,
            "Scaled blob subpool limits for Load throughput"
        );

        let blob_store = reth_node_builder::components::create_blob_store_with_cache(
            ctx,
            Some(blob_cache_size),
        )?;

        let validator = TransactionValidationTaskExecutor::eth_builder(ctx.provider().clone())
            .with_head_timestamp(ctx.head().timestamp)
            .set_eip4844(true)
            .kzg_settings(ctx.kzg_settings()?)
            .with_max_tx_input_bytes(ctx.config().txpool.max_tx_input_bytes)
            .with_local_transactions_config(pool_config.local_transactions_config.clone())
            .set_tx_fee_cap(ctx.config().rpc.rpc_tx_fee_cap)
            .with_max_tx_gas_limit(ctx.config().txpool.max_tx_gas_limit)
            .with_minimum_priority_fee(ctx.config().txpool.minimum_priority_fee)
            .with_additional_tasks(ctx.config().txpool.additional_validation_tasks)
            .build_with_tasks(ctx.task_executor().clone(), blob_store.clone());

        if validator.validator().eip4844() {
            // Initialize KZG in background to avoid first-block latency.
            let kzg_settings = validator.validator().kzg_settings().clone();
            ctx.task_executor().spawn_blocking(async move {
                let _ = kzg_settings.get();
                debug!(target: "load_reth::pool", "Initialized KZG settings");
            });
        }

        let transaction_pool = TxPoolBuilder::new(ctx)
            .with_validator(validator)
            .build_and_spawn_maintenance_task(blob_store, pool_config)?;

        let metrics = LoadBlobCacheMetrics::new();
        let pool_clone = transaction_pool.clone();
        ctx.task_executor().spawn(Box::pin(async move {
            let mut ticker = interval(Duration::from_secs(5));
            loop {
                ticker.tick().await;
                let store = pool_clone.blob_store();
                let items = store.blobs_len();
                let bytes = store.data_size_hint().map(|b| b as u64);
                metrics.record(items, bytes);
            }
        }));

        info!(target: "load_reth::pool", "Transaction pool initialized");
        Ok(transaction_pool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chainspec;

    #[test]
    fn test_blob_cache_calculation() {
        let target = chainspec::LOAD_TARGET_BLOB_COUNT;
        let cache_size = (target * EPOCH_SLOTS * 2) as u32;
        assert_eq!(cache_size, 32_768, "Cache should be 32,768 blobs for Load");
    }
}
