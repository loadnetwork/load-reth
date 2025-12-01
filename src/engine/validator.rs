//! Load-specific Engine API validator.
//!
//! Wraps the upstream Ethereum validator and enforces Load invariants on payload attributes:
//! - `prev_randao` must equal `LOAD_PREVRANDAO`.
//! - Fork-specific field validation remains delegated to the upstream validator.

use std::sync::Arc;

use eyre::eyre;
#[cfg(test)]
use reth::chainspec::{EthereumHardfork, ForkCondition};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_engine_primitives::{EngineApiValidator, EngineTypes, PayloadValidator};
use reth_ethereum_payload_builder::EthereumExecutionPayloadValidator;
use reth_ethereum_primitives::Block;
use reth_node_api::{AddOnsContext, FullNodeComponents, NodeTypes};
use reth_node_builder::rpc::PayloadValidatorBuilder;
use reth_payload_primitives::{
    EngineApiMessageVersion, EngineObjectValidationError, NewPayloadError, PayloadOrAttributes,
    PayloadTypes,
};
use reth_primitives_traits::RecoveredBlock;

use crate::{
    chainspec::LOAD_MAX_BLOB_COUNT,
    engine::payload::{LoadExecutionData, LoadPayloadAttributes},
    LOAD_PREVRANDAO,
};

/// Load engine validator wrapping the Ethereum execution payload validator.
#[derive(Debug, Clone)]
pub struct LoadEngineValidator<ChainSpec = reth_chainspec::ChainSpec> {
    inner: EthereumExecutionPayloadValidator<ChainSpec>,
}

/// Builder for the Load engine validator (used by node add-ons).
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct LoadEngineValidatorBuilder;

impl<Node, Types> PayloadValidatorBuilder<Node> for LoadEngineValidatorBuilder
where
    Types: NodeTypes<
        ChainSpec: EthChainSpec + EthereumHardforks + Clone + 'static,
        Payload: EngineTypes<ExecutionData = LoadExecutionData> + PayloadTypes,
    >,
    Node: FullNodeComponents<Types = Types>,
{
    type Validator = LoadEngineValidator<Types::ChainSpec>;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(LoadEngineValidator::new(ctx.config.chain.clone()))
    }
}

impl<ChainSpec> LoadEngineValidator<ChainSpec> {
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { inner: EthereumExecutionPayloadValidator::new(chain_spec) }
    }

    #[inline]
    fn chain_spec(&self) -> &ChainSpec {
        self.inner.chain_spec()
    }
}

impl<ChainSpec, Types> PayloadValidator<Types> for LoadEngineValidator<ChainSpec>
where
    ChainSpec: EthChainSpec + EthereumHardforks + 'static,
    Types: PayloadTypes<ExecutionData = LoadExecutionData>,
{
    type Block = Block;

    fn ensure_well_formed_payload(
        &self,
        payload: LoadExecutionData,
    ) -> Result<RecoveredBlock<Self::Block>, NewPayloadError> {
        let sealed_block = self.inner.ensure_well_formed_payload(payload.into())?;
        sealed_block.try_recover().map_err(|e| NewPayloadError::Other(e.into()))
    }
}

impl<ChainSpec, Types> EngineApiValidator<Types> for LoadEngineValidator<ChainSpec>
where
    ChainSpec: EthChainSpec + EthereumHardforks + 'static,
    Types:
        PayloadTypes<PayloadAttributes = LoadPayloadAttributes, ExecutionData = LoadExecutionData>,
{
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, Types::ExecutionData, LoadPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        // Enforce blob limits on incoming payloads (Load allows up to 1024, not 128) and
        // prev_randao constant on payloads. Also gate Prague fields to the fork activation.
        let prague_active =
            self.chain_spec().is_prague_active_at_timestamp(payload_or_attrs.timestamp());
        if let PayloadOrAttributes::ExecutionPayload(payload) = &payload_or_attrs {
            if payload.payload().prev_randao().as_slice() != LOAD_PREVRANDAO {
                return Err(EngineObjectValidationError::InvalidParams(
                    eyre!("prev_randao must be constant 0x01 for Load").into(),
                ));
            }

            if let Some(versioned_hashes) = payload.sidecar().versioned_hashes() {
                if versioned_hashes.len() > LOAD_MAX_BLOB_COUNT as usize {
                    return Err(EngineObjectValidationError::InvalidParams(
                        eyre!(
                            "too many blob versioned hashes: {} (max {})",
                            versioned_hashes.len(),
                            LOAD_MAX_BLOB_COUNT
                        )
                        .into(),
                    ));
                }
            }

            if !prague_active && payload.sidecar().requests().is_some() {
                return Err(EngineObjectValidationError::InvalidParams(
                    eyre!(
                        "Prague payload fields not active at timestamp {}",
                        payload.payload().timestamp()
                    )
                    .into(),
                ));
            }
        }

        // Enforce prev_randao invariant on attributes.
        if let PayloadOrAttributes::PayloadAttributes(attrs) = &payload_or_attrs {
            if attrs.prev_randao().as_slice() != LOAD_PREVRANDAO {
                return Err(EngineObjectValidationError::InvalidParams(
                    eyre!("prev_randao must be constant 0x01 for Load").into(),
                ));
            }
        }

        reth_payload_primitives::validate_version_specific_fields(
            self.chain_spec(),
            version,
            payload_or_attrs,
        )
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &LoadPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        if attributes.prev_randao().as_slice() != LOAD_PREVRANDAO {
            return Err(EngineObjectValidationError::InvalidParams(
                eyre!("prev_randao must be constant 0x01 for Load").into(),
            ));
        }

        reth_payload_primitives::validate_version_specific_fields(
            self.chain_spec(),
            version,
            PayloadOrAttributes::<Types::ExecutionData, LoadPayloadAttributes>::PayloadAttributes(
                attributes,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy_eips::eip7685::{Requests, RequestsOrHash};
    use alloy_primitives::{Address, Bloom, Bytes, U256};
    use alloy_rpc_types_engine::{
        CancunPayloadFields, ExecutionPayload, ExecutionPayloadSidecar, ExecutionPayloadV1,
        ExecutionPayloadV2, ExecutionPayloadV3,
    };

    use super::*;
    use crate::{chainspec::LoadChainSpec, engine::payload::LoadEngineTypes};

    #[test]
    fn build_validator() {
        let spec = Arc::new(LoadChainSpec::default());
        let validator = LoadEngineValidator::new(spec);
        assert!(validator.chain_spec().genesis_hash() != alloy_primitives::B256::ZERO);
    }

    #[test]
    fn rejects_payload_with_too_many_versioned_hashes() {
        let payload_v1 = ExecutionPayloadV1 {
            parent_hash: Default::default(),
            fee_recipient: Address::ZERO,
            state_root: Default::default(),
            receipts_root: Default::default(),
            logs_bloom: Bloom::default(),
            prev_randao: Default::default(),
            block_number: 1,
            gas_limit: 30_000_000,
            gas_used: 0,
            timestamp: 1,
            extra_data: Bytes::default(),
            base_fee_per_gas: U256::ZERO,
            block_hash: Default::default(),
            transactions: Vec::new(),
        };

        let payload_v2 = ExecutionPayloadV2 { payload_inner: payload_v1, withdrawals: Vec::new() };
        let payload_v3 =
            ExecutionPayloadV3 { payload_inner: payload_v2, blob_gas_used: 0, excess_blob_gas: 0 };

        let versioned_hashes = vec![
            alloy_primitives::B256::ZERO;
            (crate::chainspec::LOAD_MAX_BLOB_COUNT as usize) + 1
        ];
        let sidecar = ExecutionPayloadSidecar::v3(CancunPayloadFields {
            parent_beacon_block_root: alloy_primitives::B256::ZERO,
            versioned_hashes,
        });

        let execution_data = LoadExecutionData::new(ExecutionPayload::V3(payload_v3), sidecar);

        let validator = LoadEngineValidator::new(Arc::new(LoadChainSpec::default()));
        let result = <LoadEngineValidator<LoadChainSpec> as EngineApiValidator<
            LoadEngineTypes,
        >>::validate_version_specific_fields(
            &validator,
            EngineApiMessageVersion::V3,
            PayloadOrAttributes::from_execution_payload(&execution_data),
        );

        assert!(matches!(result, Err(EngineObjectValidationError::InvalidParams(_))));
    }

    #[test]
    fn rejects_payload_with_wrong_prev_randao() {
        let payload_v1 = ExecutionPayloadV1 {
            parent_hash: Default::default(),
            fee_recipient: Address::ZERO,
            state_root: Default::default(),
            receipts_root: Default::default(),
            logs_bloom: Bloom::default(),
            prev_randao: alloy_primitives::B256::ZERO, // wrong
            block_number: 1,
            gas_limit: 30_000_000,
            gas_used: 0,
            timestamp: 1,
            extra_data: Bytes::default(),
            base_fee_per_gas: U256::ZERO,
            block_hash: Default::default(),
            transactions: Vec::new(),
        };

        let payload_v2 = ExecutionPayloadV2 { payload_inner: payload_v1, withdrawals: Vec::new() };
        let payload_v3 =
            ExecutionPayloadV3 { payload_inner: payload_v2, blob_gas_used: 0, excess_blob_gas: 0 };

        let sidecar = ExecutionPayloadSidecar::v3(CancunPayloadFields {
            parent_beacon_block_root: alloy_primitives::B256::ZERO,
            versioned_hashes: Vec::new(),
        });

        let execution_data = LoadExecutionData::new(ExecutionPayload::V3(payload_v3), sidecar);

        let mut spec = LoadChainSpec::default();
        spec.inner.hardforks.insert(EthereumHardfork::Prague, ForkCondition::Timestamp(1_000));
        let validator = LoadEngineValidator::new(Arc::new(spec));
        let result = <LoadEngineValidator<LoadChainSpec> as EngineApiValidator<
            LoadEngineTypes,
        >>::validate_version_specific_fields(
            &validator,
            EngineApiMessageVersion::V3,
            PayloadOrAttributes::from_execution_payload(&execution_data),
        );

        assert!(matches!(result, Err(EngineObjectValidationError::InvalidParams(_))));
    }

    #[test]
    fn prague_fields_rejected_before_activation() {
        let payload_v1 = ExecutionPayloadV1 {
            parent_hash: Default::default(),
            fee_recipient: Address::ZERO,
            state_root: Default::default(),
            receipts_root: Default::default(),
            logs_bloom: Bloom::default(),
            prev_randao: alloy_primitives::B256::from(crate::LOAD_PREVRANDAO),
            block_number: 1,
            gas_limit: 30_000_000,
            gas_used: 0,
            timestamp: 1,
            extra_data: Bytes::default(),
            base_fee_per_gas: U256::ZERO,
            block_hash: Default::default(),
            transactions: Vec::new(),
        };

        let payload_v2 = ExecutionPayloadV2 { payload_inner: payload_v1, withdrawals: Vec::new() };
        let payload_v3 =
            ExecutionPayloadV3 { payload_inner: payload_v2, blob_gas_used: 0, excess_blob_gas: 0 };

        let execution_data = LoadExecutionData::v4(
            payload_v3,
            Vec::new(),
            alloy_primitives::B256::ZERO,
            RequestsOrHash::Requests(Requests::default()),
        );

        let mut spec = LoadChainSpec::default();
        spec.inner.hardforks.insert(EthereumHardfork::Prague, ForkCondition::Timestamp(1_000));
        assert!(!spec.is_prague_active_at_timestamp(1));

        let validator = LoadEngineValidator::new(Arc::new(spec));
        let result = <LoadEngineValidator<LoadChainSpec> as EngineApiValidator<
            LoadEngineTypes,
        >>::validate_version_specific_fields(
            &validator,
            EngineApiMessageVersion::V3,
            PayloadOrAttributes::from_execution_payload(&execution_data),
        );

        assert!(matches!(result, Err(EngineObjectValidationError::InvalidParams(_))));
    }

    #[test]
    fn prague_requests_allowed_after_activation() {
        let payload_v1 = ExecutionPayloadV1 {
            parent_hash: Default::default(),
            fee_recipient: Address::ZERO,
            state_root: Default::default(),
            receipts_root: Default::default(),
            logs_bloom: Bloom::default(),
            prev_randao: alloy_primitives::B256::from(crate::LOAD_PREVRANDAO),
            block_number: 1,
            gas_limit: 30_000_000,
            gas_used: 0,
            timestamp: 1,
            extra_data: Bytes::default(),
            base_fee_per_gas: U256::ZERO,
            block_hash: Default::default(),
            transactions: Vec::new(),
        };

        let payload_v2 = ExecutionPayloadV2 { payload_inner: payload_v1, withdrawals: Vec::new() };
        let payload_v3 =
            ExecutionPayloadV3 { payload_inner: payload_v2, blob_gas_used: 0, excess_blob_gas: 0 };

        let execution_data = LoadExecutionData::v4(
            payload_v3,
            Vec::new(),
            alloy_primitives::B256::ZERO,
            RequestsOrHash::Requests(Requests::default()),
        );

        let spec = LoadChainSpec::default();
        assert!(spec.is_prague_active_at_timestamp(1));
        let validator = LoadEngineValidator::new(Arc::new(spec));
        let result = <LoadEngineValidator<LoadChainSpec> as EngineApiValidator<
            LoadEngineTypes,
        >>::validate_version_specific_fields(
            &validator,
            EngineApiMessageVersion::V4,
            PayloadOrAttributes::from_execution_payload(&execution_data),
        );

        assert!(result.is_ok());
    }
}
