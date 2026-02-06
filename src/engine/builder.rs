//! Load-specific payload builder implementation.
//!
//! Mirrors the upstream Ethereum builder but clamps blob selection to the Load
//! constants (1024 blobs per block) and uses the Load payload wrapper types.

use std::sync::Arc;

use alloy_consensus::Transaction;
use alloy_rlp::Encodable;
use reth::{
    api::{FullNodeTypes, NodeTypes, PayloadBuilderError, PayloadTypes, TxTy},
    consensus::ConsensusError,
    providers::{ChainSpecProvider, StateProviderFactory},
    revm::{context::Block, database::StateProviderDatabase, State},
    transaction_pool::{PoolTransaction, TransactionPool},
};
use reth_basic_payload_builder::{
    is_better_payload, BuildArguments, BuildOutcome, HeaderForPayload, MissingPayloadBehaviour,
    PayloadBuilder, PayloadConfig,
};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_consensus_common::validation::MAX_RLP_BLOCK_SIZE;
use reth_ethereum_engine_primitives::{BlobSidecars, EthBuiltPayload};
use reth_ethereum_payload_builder::EthereumBuilderConfig;
use reth_evm::{
    block::{BlockExecutionError, BlockValidationError},
    execute::{BlockBuilder, BlockBuilderOutcome},
    ConfigureEvm, Evm,
};
use reth_node_builder::{components::PayloadBuilderBuilder, BuilderContext, PayloadBuilderConfig};
use reth_payload_primitives::PayloadBuilderAttributes;
use reth_primitives_traits::transaction::error::InvalidTransactionError;
use reth_transaction_pool::{
    error::{Eip4844PoolTransactionError, InvalidPoolTransactionError},
    BestTransactions, BestTransactionsAttributes, ValidPoolTransaction,
};
use tracing::{debug, trace, warn};

use crate::{
    chainspec::{LoadChainSpec, LOAD_EXECUTION_GAS_LIMIT, LOAD_MAX_BLOB_COUNT},
    engine::payload::{LoadBuiltPayload, LoadPayloadBuilderAttributes},
    primitives::LoadPrimitives,
};

type BestTransactionsIter<Pool> = Box<
    dyn BestTransactions<Item = Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>,
>;

/// Service builder for Load payload builders.
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct LoadPayloadServiceBuilder;

impl<Types, Node, Pool, Evm> PayloadBuilderBuilder<Node, Pool, Evm> for LoadPayloadServiceBuilder
where
    Types: NodeTypes<ChainSpec = LoadChainSpec, Primitives = LoadPrimitives>,
    Node: FullNodeTypes<Types = Types>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Clone
        + Unpin
        + 'static,
    Evm: ConfigureEvm<
            Primitives = reth_node_api::PrimitivesTy<Types>,
            NextBlockEnvCtx = reth_evm::NextBlockEnvAttributes,
        > + Clone
        + 'static,
    Types::Payload: PayloadTypes<
        BuiltPayload = LoadBuiltPayload,
        PayloadAttributes = crate::engine::payload::LoadPayloadAttributes,
        PayloadBuilderAttributes = LoadPayloadBuilderAttributes,
    >,
{
    type PayloadBuilder = LoadPayloadBuilder<Pool, Node::Provider, Evm>;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: Evm,
    ) -> eyre::Result<Self::PayloadBuilder> {
        let conf = ctx.payload_builder_config();
        // Use Load's 2B gas limit instead of reth's 36M default for custom chains
        let gas_limit = conf.gas_limit().unwrap_or(LOAD_EXECUTION_GAS_LIMIT);

        let builder_config = EthereumBuilderConfig::new()
            .with_gas_limit(gas_limit)
            .with_await_payload_on_missing(true);

        Ok(LoadPayloadBuilder::new(ctx.provider().clone(), pool, evm_config, builder_config))
    }
}

/// Load payload builder that enforces blob caps during transaction selection.
#[derive(Debug, Clone)]
pub struct LoadPayloadBuilder<Pool, Client, EvmConfig = crate::evm::LoadEvmConfig> {
    client: Client,
    pool: Pool,
    evm_config: EvmConfig,
    builder_config: EthereumBuilderConfig,
}

impl<Pool, Client, EvmConfig> LoadPayloadBuilder<Pool, Client, EvmConfig> {
    pub const fn new(
        client: Client,
        pool: Pool,
        evm_config: EvmConfig,
        builder_config: EthereumBuilderConfig,
    ) -> Self {
        Self { client, pool, evm_config, builder_config }
    }
}

impl<Pool, Client, EvmConfig> PayloadBuilder for LoadPayloadBuilder<Pool, Client, EvmConfig>
where
    EvmConfig: ConfigureEvm<
            Primitives = LoadPrimitives,
            NextBlockEnvCtx = reth_evm::NextBlockEnvAttributes,
        > + Clone
        + 'static,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = LoadChainSpec> + Clone,
    Pool: TransactionPool<
            Transaction: PoolTransaction<Consensus = reth_ethereum_primitives::TransactionSigned>,
        > + Clone,
{
    type Attributes = LoadPayloadBuilderAttributes;
    type BuiltPayload = LoadBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        default_load_payload(
            self.evm_config.clone(),
            self.client.clone(),
            self.pool.clone(),
            self.builder_config.clone(),
            args,
            |attributes| self.pool.best_transactions_with_attributes(attributes),
        )
    }

    fn on_missing_payload(
        &self,
        _args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        if self.builder_config.await_payload_on_missing {
            MissingPayloadBehaviour::AwaitInProgress
        } else {
            MissingPayloadBehaviour::RaceEmptyPayload
        }
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes, HeaderForPayload<Self::BuiltPayload>>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let args = BuildArguments::new(Default::default(), config, Default::default(), None);
        default_load_payload(
            self.evm_config.clone(),
            self.client.clone(),
            self.pool.clone(),
            self.builder_config.clone(),
            args,
            |attributes| self.pool.best_transactions_with_attributes(attributes),
        )?
        .into_payload()
        .ok_or(PayloadBuilderError::MissingPayload)
    }
}

/// Load-specific payload building loop (clamps blob selection to 1024).
#[allow(clippy::too_many_arguments)]
pub fn default_load_payload<EvmConfig, Client, Pool, F>(
    evm_config: EvmConfig,
    client: Client,
    pool: Pool,
    builder_config: EthereumBuilderConfig,
    args: BuildArguments<LoadPayloadBuilderAttributes, LoadBuiltPayload>,
    best_txs: F,
) -> Result<BuildOutcome<LoadBuiltPayload>, PayloadBuilderError>
where
    EvmConfig: ConfigureEvm<
        Primitives = LoadPrimitives,
        NextBlockEnvCtx = reth_evm::NextBlockEnvAttributes,
    >,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = LoadChainSpec>,
    Pool: TransactionPool<
        Transaction: PoolTransaction<Consensus = reth_ethereum_primitives::TransactionSigned>,
    >,
    F: FnOnce(BestTransactionsAttributes) -> BestTransactionsIter<Pool>,
{
    let BuildArguments { mut cached_reads, config, cancel, best_payload } = args;
    let PayloadConfig { parent_header, attributes } = config;

    super::payload::validate_payload_attributes(parent_header.timestamp, &attributes)
        .map_err(PayloadBuilderError::other)?;

    let state_provider = client.state_by_block_hash(parent_header.hash())?;
    let state = StateProviderDatabase::new(&state_provider);
    let mut db =
        State::builder().with_database(cached_reads.as_db_mut(state)).with_bundle_update().build();

    let chain_spec = client.chain_spec();
    let extra_data = chain_spec.inner.genesis.extra_data.clone();

    let mut builder = evm_config
        .builder_for_next_block(
            &mut db,
            &parent_header,
            reth_evm::NextBlockEnvAttributes {
                timestamp: attributes.timestamp(),
                suggested_fee_recipient: attributes.suggested_fee_recipient(),
                prev_randao: attributes.prev_randao(),
                gas_limit: builder_config.gas_limit(parent_header.gas_limit),
                parent_beacon_block_root: attributes.parent_beacon_block_root(),
                withdrawals: Some(attributes.withdrawals().clone()),
                extra_data,
            },
        )
        .map_err(PayloadBuilderError::other)?;

    debug!(
        target: "payload_builder",
        id=%attributes.payload_id(),
        parent_header = ?parent_header.hash(),
        parent_number = parent_header.number,
        "building new load payload"
    );

    let mut cumulative_gas_used = 0;
    let block_gas_limit: u64 = builder.evm_mut().block().gas_limit();
    let base_fee = builder.evm_mut().block().basefee();

    let mut best_txs = best_txs(BestTransactionsAttributes::new(
        base_fee,
        builder.evm_mut().block().blob_gasprice().map(|gasprice| gasprice as u64),
    ));
    let mut total_fees = alloy_primitives::U256::ZERO;

    builder.apply_pre_execution_changes().map_err(|err| {
        warn!(target: "payload_builder", %err, "failed to apply pre-execution changes");
        PayloadBuilderError::Internal(err.into())
    })?;

    let mut blob_sidecars = BlobSidecars::Empty;
    let mut block_blob_count = 0;
    let mut block_transactions_rlp_length = 0;

    let max_blob_count = compute_load_blob_cap(chain_spec.as_ref(), attributes.timestamp());

    let is_osaka = chain_spec.is_osaka_active_at_timestamp(attributes.timestamp());

    while let Some(pool_tx) = best_txs.next() {
        if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
            best_txs.mark_invalid(
                &pool_tx,
                &InvalidPoolTransactionError::ExceedsGasLimit(pool_tx.gas_limit(), block_gas_limit),
            );
            continue;
        }

        if cancel.is_cancelled() {
            return Ok(BuildOutcome::Cancelled);
        }

        let tx = pool_tx.to_consensus();

        let estimated_block_size_with_tx = block_transactions_rlp_length +
            tx.inner().length() +
            attributes.withdrawals().length() +
            1024;

        if is_osaka && estimated_block_size_with_tx > MAX_RLP_BLOCK_SIZE {
            best_txs.mark_invalid(
                &pool_tx,
                &InvalidPoolTransactionError::OversizedData {
                    size: estimated_block_size_with_tx,
                    limit: MAX_RLP_BLOCK_SIZE,
                },
            );
            continue;
        }

        let mut blob_tx_sidecar = None;
        if let Some(blob_tx) = tx.as_eip4844() {
            let tx_blob_count = blob_tx.tx().blob_versioned_hashes.len() as u64;

            if block_blob_count + tx_blob_count > max_blob_count {
                trace!(
                    target: "payload_builder",
                    tx=?tx.hash(),
                    ?block_blob_count,
                    "skipping blob transaction because it would exceed load blob cap"
                );
                best_txs.mark_invalid(
                    &pool_tx,
                    &InvalidPoolTransactionError::Eip4844(
                        Eip4844PoolTransactionError::TooManyEip4844Blobs {
                            have: block_blob_count + tx_blob_count,
                            permitted: max_blob_count,
                        },
                    ),
                );
                continue;
            }

            let blob_sidecar_result = 'sidecar: {
                let Some(sidecar) =
                    pool.get_blob(*tx.hash()).map_err(PayloadBuilderError::other)?
                else {
                    break 'sidecar Err(Eip4844PoolTransactionError::MissingEip4844BlobSidecar);
                };

                if is_osaka {
                    if sidecar.is_eip7594() {
                        Ok(sidecar)
                    } else {
                        Err(Eip4844PoolTransactionError::UnexpectedEip4844SidecarAfterOsaka)
                    }
                } else if sidecar.is_eip4844() {
                    Ok(sidecar)
                } else {
                    Err(Eip4844PoolTransactionError::UnexpectedEip7594SidecarBeforeOsaka)
                }
            };

            blob_tx_sidecar = match blob_sidecar_result {
                Ok(sidecar) => Some(sidecar),
                Err(error) => {
                    best_txs.mark_invalid(&pool_tx, &InvalidPoolTransactionError::Eip4844(error));
                    continue;
                }
            };
        }

        let gas_used = match builder.execute_transaction(tx.clone()) {
            Ok(gas_used) => gas_used,
            Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                error, ..
            })) => {
                if error.is_nonce_too_low() {
                    trace!(target: "payload_builder", %error, ?tx, "skipping nonce too low transaction");
                } else {
                    trace!(target: "payload_builder", %error, ?tx, "skipping invalid transaction and its descendants");
                    best_txs.mark_invalid(
                        &pool_tx,
                        &InvalidPoolTransactionError::Consensus(
                            InvalidTransactionError::TxTypeNotSupported,
                        ),
                    );
                }
                continue;
            }
            Err(err) => return Err(PayloadBuilderError::evm(err)),
        };

        if let Some(blob_tx) = tx.as_eip4844() {
            block_blob_count += blob_tx.tx().blob_versioned_hashes.len() as u64;
            if block_blob_count == max_blob_count {
                best_txs.skip_blobs();
            }
        }

        block_transactions_rlp_length += tx.inner().length();

        let miner_fee =
            tx.effective_tip_per_gas(base_fee).expect("fee is valid after successful execution");
        total_fees +=
            alloy_primitives::U256::from(miner_fee) * alloy_primitives::U256::from(gas_used);
        cumulative_gas_used += gas_used;

        if let Some(sidecar) = blob_tx_sidecar {
            blob_sidecars.push_sidecar_variant(sidecar.as_ref().clone());
        }
    }

    if !is_better_payload(best_payload.as_ref(), total_fees) {
        drop(builder);
        return Ok(BuildOutcome::Aborted { fees: total_fees, cached_reads });
    }

    let BlockBuilderOutcome { execution_result, block, .. } = builder.finish(&state_provider)?;

    let requests = chain_spec
        .is_prague_active_at_timestamp(attributes.timestamp())
        .then_some(execution_result.requests);

    let sealed_block = Arc::new(block.sealed_block().clone());
    debug!(
        target: "payload_builder",
        id=%attributes.payload_id(),
        sealed_block_header = ?sealed_block.sealed_header(),
        "sealed load payload"
    );

    if is_osaka && sealed_block.rlp_length() > MAX_RLP_BLOCK_SIZE {
        return Err(PayloadBuilderError::other(ConsensusError::BlockTooLarge {
            rlp_length: sealed_block.rlp_length(),
            max_rlp_length: MAX_RLP_BLOCK_SIZE,
        }));
    }

    let payload = EthBuiltPayload::new(attributes.payload_id(), sealed_block, total_fees, requests)
        .with_sidecars(blob_sidecars);

    Ok(BuildOutcome::Better { payload: LoadBuiltPayload::new(payload), cached_reads })
}

fn compute_load_blob_cap(chain_spec: &LoadChainSpec, timestamp: u64) -> u64 {
    chain_spec
        .blob_params_at_timestamp(timestamp)
        .map(|params| params.max_blob_count)
        .unwrap_or(LOAD_MAX_BLOB_COUNT)
        .min(LOAD_MAX_BLOB_COUNT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_cap_clamped_to_load_constant() {
        let mut spec = LoadChainSpec::default();
        spec.inner.blob_params.cancun.max_blob_count = super::LOAD_MAX_BLOB_COUNT * 2;
        let cap = compute_load_blob_cap(&spec, spec.inner.genesis().timestamp);
        assert_eq!(cap, super::LOAD_MAX_BLOB_COUNT);
    }

    #[test]
    fn blob_cap_defaults_to_constant() {
        let spec = LoadChainSpec::default();
        let cap = compute_load_blob_cap(&spec, spec.inner.genesis().timestamp);
        assert_eq!(cap, super::LOAD_MAX_BLOB_COUNT);
    }
}
