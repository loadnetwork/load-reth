//! Load payload helpers and wrappers.
//!
//! Enforces Load invariants on top of standard Ethereum payloads:
//! - `prev_randao` must be the constant `0x01`.
//! - Only EIP-4844 sidecars accepted (EIP-7594 rejected).
//! - Blob count capped at 1024.

use std::{fmt, sync::Arc};

use alloy_eips::{eip4895::Withdrawals, eip7685::RequestsOrHash};
use alloy_primitives::B256;
use alloy_rpc_types_engine::{
    BlobsBundleV1, CancunPayloadFields, ExecutionData, ExecutionPayload,
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
    ExecutionPayloadEnvelopeV5, ExecutionPayloadInputV2, ExecutionPayloadSidecar,
    ExecutionPayloadV1, ExecutionPayloadV3, PayloadAttributes as EthPayloadAttributes, PayloadId,
    PraguePayloadFields,
};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_ethereum_engine_primitives::{
    BlobSidecars, BuiltPayloadConversionError, EthBuiltPayload, EthPayloadBuilderAttributes,
};
use reth_evm::{ConfigureEngineEvm, EvmEnvFor, ExecutableTxIterator, ExecutionCtxFor};
use reth_payload_primitives::{
    BuiltPayload, ExecutionPayload as ExecutionPayloadTrait, PayloadAttributesBuilder,
    PayloadBuilderAttributes,
};
use reth_primitives_traits::{NodePrimitives, SealedBlock};
use thiserror::Error;

use crate::{chainspec::LOAD_MAX_BLOB_COUNT, LOAD_PREVRANDAO};

/// Validate `prev_randao` for Load (must be constant 0x01).
pub fn validate_prev_randao(prev_randao: B256) -> Result<(), String> {
    if prev_randao.as_slice() == LOAD_PREVRANDAO {
        Ok(())
    } else {
        Err("prev_randao must be constant 0x01 for Load".to_string())
    }
}

/// Validation errors for Load payload attributes.
#[derive(Debug, Error)]
pub enum LoadPayloadAttributesError {
    #[error("invalid prev_randao for Load: {0}")]
    InvalidPrevRandao(String),
    #[error("invalid timestamp for Load: {0}")]
    InvalidTimestamp(String),
}

/// Load RPC payload attributes (wrapper over Ethereum attributes).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoadPayloadAttributes {
    #[serde(flatten)]
    pub inner: EthPayloadAttributes,
}

impl LoadPayloadAttributes {
    pub fn prev_randao(&self) -> B256 {
        self.inner.prev_randao
    }

    pub fn timestamp(&self) -> u64 {
        self.inner.timestamp
    }
}

impl reth::api::PayloadAttributes for LoadPayloadAttributes {
    fn timestamp(&self) -> u64 {
        self.inner.timestamp
    }

    fn withdrawals(&self) -> Option<&Vec<alloy_eips::eip4895::Withdrawal>> {
        self.inner.withdrawals.as_ref()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.inner.parent_beacon_block_root
    }
}

/// Load payload builder attributes that validate Load invariants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadPayloadBuilderAttributes {
    inner: EthPayloadBuilderAttributes,
}

impl LoadPayloadBuilderAttributes {
    pub const fn new(inner: EthPayloadBuilderAttributes) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> EthPayloadBuilderAttributes {
        self.inner
    }
}

impl reth_payload_primitives::PayloadBuilderAttributes for LoadPayloadBuilderAttributes {
    type RpcPayloadAttributes = LoadPayloadAttributes;
    type Error = LoadPayloadAttributesError;

    fn try_new(
        parent: B256,
        rpc_payload_attributes: LoadPayloadAttributes,
        version: u8,
    ) -> Result<Self, Self::Error> {
        let inner =
            EthPayloadBuilderAttributes::try_new(parent, rpc_payload_attributes.inner, version)
                .expect("EthPayloadBuilderAttributes::try_new is infallible");

        validate_prev_randao(inner.prev_randao())
            .map_err(LoadPayloadAttributesError::InvalidPrevRandao)?;

        Ok(Self::new(inner))
    }

    fn payload_id(&self) -> PayloadId {
        self.inner.payload_id()
    }

    fn parent(&self) -> B256 {
        self.inner.parent()
    }

    fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.inner.parent_beacon_block_root()
    }

    fn suggested_fee_recipient(&self) -> alloy_primitives::Address {
        self.inner.suggested_fee_recipient()
    }

    fn prev_randao(&self) -> B256 {
        self.inner.prev_randao()
    }

    fn withdrawals(&self) -> &Withdrawals {
        self.inner.withdrawals()
    }
}

/// Validate Load payload attributes against parent header fields.
pub fn validate_payload_attributes(
    parent_timestamp: u64,
    attrs: &LoadPayloadBuilderAttributes,
) -> Result<(), LoadPayloadAttributesError> {
    validate_prev_randao(attrs.prev_randao())
        .map_err(LoadPayloadAttributesError::InvalidPrevRandao)?;

    if attrs.timestamp() <= parent_timestamp {
        return Err(LoadPayloadAttributesError::InvalidTimestamp(format!(
            "timestamp {} must be greater than parent {}",
            attrs.timestamp(),
            parent_timestamp,
        )));
    }

    Ok(())
}

/// Local payload attributes builder for Load (debug/self-builders).
pub struct LoadLocalPayloadAttributesBuilder(
    pub reth_engine_local::LocalPayloadAttributesBuilder<crate::chainspec::LoadChainSpec>,
);

impl fmt::Debug for LoadLocalPayloadAttributesBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("LoadLocalPayloadAttributesBuilder")
            .field(&self.0.chain_spec.chain())
            .finish()
    }
}

impl Clone for LoadLocalPayloadAttributesBuilder {
    fn clone(&self) -> Self {
        Self(reth_engine_local::LocalPayloadAttributesBuilder::new(self.0.chain_spec.clone()))
    }
}

impl PayloadAttributesBuilder<LoadPayloadAttributes> for LoadLocalPayloadAttributesBuilder {
    fn build(&self, timestamp: u64) -> LoadPayloadAttributes {
        LoadPayloadAttributes {
            inner: EthPayloadAttributes {
                timestamp,
                prev_randao: B256::from(crate::LOAD_PREVRANDAO),
                suggested_fee_recipient: alloy_primitives::Address::ZERO,
                withdrawals: self
                    .0
                    .chain_spec
                    .is_shanghai_active_at_timestamp(timestamp)
                    .then(Default::default),
                parent_beacon_block_root: self
                    .0
                    .chain_spec
                    .is_cancun_active_at_timestamp(timestamp)
                    .then(B256::random),
            },
        }
    }
}

/// Load-specific built payload wrapper.
#[derive(Debug, Clone)]
pub struct LoadBuiltPayload {
    pub inner: EthBuiltPayload,
}

impl LoadBuiltPayload {
    pub const fn new(inner: EthBuiltPayload) -> Self {
        Self { inner }
    }

    fn guard_sidecars(&self, max_blobs: usize) -> Result<(), BuiltPayloadConversionError> {
        match self.inner.sidecars() {
            BlobSidecars::Eip7594(_) => Err(BuiltPayloadConversionError::UnexpectedEip7594Sidecars),
            BlobSidecars::Eip4844(sidecars) if sidecars.len() > max_blobs => {
                Err(BuiltPayloadConversionError::UnexpectedEip4844Sidecars)
            }
            _ => Ok(()),
        }
    }
}

impl BuiltPayload for LoadBuiltPayload {
    type Primitives = <EthBuiltPayload as BuiltPayload>::Primitives;

    fn block(&self) -> &SealedBlock<<Self::Primitives as NodePrimitives>::Block> {
        self.inner.block()
    }

    fn fees(&self) -> alloy_primitives::U256 {
        self.inner.fees()
    }

    fn requests(&self) -> Option<alloy_eips::eip7685::Requests> {
        self.inner.requests()
    }
}

impl From<LoadBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: LoadBuiltPayload) -> Self {
        ExecutionPayloadV1::from(value.inner)
    }
}

impl From<LoadBuiltPayload> for ExecutionPayloadEnvelopeV2 {
    fn from(value: LoadBuiltPayload) -> Self {
        ExecutionPayloadEnvelopeV2::from(value.inner)
    }
}

impl TryFrom<LoadBuiltPayload> for ExecutionPayloadEnvelopeV3 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: LoadBuiltPayload) -> Result<Self, Self::Error> {
        value.guard_sidecars(LOAD_MAX_BLOB_COUNT as usize)?;
        let blobs_bundle = match value.inner.sidecars() {
            BlobSidecars::Empty => BlobsBundleV1::empty(),
            BlobSidecars::Eip4844(sidecars) => BlobsBundleV1::from(sidecars.clone()),
            BlobSidecars::Eip7594(_) => unreachable!(),
        };

        Ok(ExecutionPayloadEnvelopeV3 {
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                value.block().hash(),
                &value.block().clone().into_block(),
            ),
            block_value: value.fees(),
            should_override_builder: false,
            blobs_bundle,
        })
    }
}

impl TryFrom<LoadBuiltPayload> for ExecutionPayloadEnvelopeV4 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: LoadBuiltPayload) -> Result<Self, Self::Error> {
        value.guard_sidecars(LOAD_MAX_BLOB_COUNT as usize)?;
        ExecutionPayloadEnvelopeV4::try_from(value.inner)
    }
}

impl TryFrom<LoadBuiltPayload> for ExecutionPayloadEnvelopeV5 {
    type Error = BuiltPayloadConversionError;

    fn try_from(value: LoadBuiltPayload) -> Result<Self, Self::Error> {
        value.guard_sidecars(LOAD_MAX_BLOB_COUNT as usize)?;
        ExecutionPayloadEnvelopeV5::try_from(value.inner)
    }
}

/// Execution payload wrapper for Load Network.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LoadExecutionData {
    inner: ExecutionData,
}

impl LoadExecutionData {
    pub fn new(payload: ExecutionPayload, sidecar: ExecutionPayloadSidecar) -> Self {
        Self { inner: ExecutionData { payload, sidecar } }
    }

    pub fn v3(
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> Self {
        Self {
            inner: ExecutionData {
                payload: payload.into(),
                sidecar: ExecutionPayloadSidecar::v3(CancunPayloadFields {
                    versioned_hashes,
                    parent_beacon_block_root,
                }),
            },
        }
    }

    pub fn v4(
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: RequestsOrHash,
    ) -> Self {
        Self {
            inner: ExecutionData {
                payload: payload.into(),
                sidecar: ExecutionPayloadSidecar::v4(
                    CancunPayloadFields { versioned_hashes, parent_beacon_block_root },
                    PraguePayloadFields::new(execution_requests),
                ),
            },
        }
    }

    pub fn into_parts(self) -> (ExecutionPayload, ExecutionPayloadSidecar) {
        let ExecutionData { payload, sidecar } = self.inner;
        (payload, sidecar)
    }

    pub fn sidecar(&self) -> &ExecutionPayloadSidecar {
        &self.inner.sidecar
    }

    pub fn payload(&self) -> &ExecutionPayload {
        &self.inner.payload
    }

    pub fn inner(&self) -> &ExecutionData {
        &self.inner
    }
}

impl ExecutionPayloadTrait for LoadExecutionData {
    fn parent_hash(&self) -> B256 {
        self.inner.payload.parent_hash()
    }

    fn block_hash(&self) -> B256 {
        self.inner.payload.block_hash()
    }

    fn block_number(&self) -> u64 {
        self.inner.payload.block_number()
    }

    fn withdrawals(&self) -> Option<&Vec<alloy_eips::eip4895::Withdrawal>> {
        self.inner.payload.withdrawals()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.inner.sidecar.parent_beacon_block_root()
    }

    fn timestamp(&self) -> u64 {
        self.inner.payload.timestamp()
    }

    fn gas_used(&self) -> u64 {
        self.inner.payload.as_v1().gas_used
    }
}

impl From<ExecutionPayloadV1> for LoadExecutionData {
    fn from(payload: ExecutionPayloadV1) -> Self {
        Self {
            inner: ExecutionData {
                payload: payload.into(),
                sidecar: ExecutionPayloadSidecar::none(),
            },
        }
    }
}

impl From<ExecutionPayloadInputV2> for LoadExecutionData {
    fn from(payload: ExecutionPayloadInputV2) -> Self {
        let payload = payload.into_payload();
        Self { inner: ExecutionData { payload, sidecar: ExecutionPayloadSidecar::none() } }
    }
}

impl From<LoadExecutionData> for ExecutionData {
    fn from(value: LoadExecutionData) -> Self {
        value.inner
    }
}

/// Convenience constructor for an empty Load payload (used in tests/examples).
pub fn empty_load_payload(id: PayloadId) -> LoadBuiltPayload {
    LoadBuiltPayload::new(EthBuiltPayload::new(
        id,
        Arc::new(SealedBlock::default()),
        alloy_primitives::U256::ZERO,
        None,
    ))
}

/// Load engine types (payload + execution metadata).
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
pub struct LoadEngineTypes;

impl reth_payload_primitives::PayloadTypes for LoadEngineTypes {
    type BuiltPayload = LoadBuiltPayload;
    type PayloadAttributes = LoadPayloadAttributes;
    type PayloadBuilderAttributes = LoadPayloadBuilderAttributes;
    type ExecutionData = LoadExecutionData;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData {
        let (payload, sidecar) =
            ExecutionPayload::from_block_unchecked(block.hash(), &block.into_block());
        LoadExecutionData::new(payload, sidecar)
    }
}

impl reth::api::EngineTypes for LoadEngineTypes {
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV4;
    type ExecutionPayloadEnvelopeV5 = ExecutionPayloadEnvelopeV5;
}

impl<ChainSpec, EvmF> ConfigureEngineEvm<LoadExecutionData>
    for reth_node_ethereum::EthEvmConfig<ChainSpec, EvmF>
where
    reth_node_ethereum::EthEvmConfig<ChainSpec, EvmF>: ConfigureEngineEvm<ExecutionData>,
{
    fn evm_env_for_payload(
        &self,
        payload: &LoadExecutionData,
    ) -> Result<EvmEnvFor<Self>, Self::Error> {
        <Self as ConfigureEngineEvm<ExecutionData>>::evm_env_for_payload(self, payload.inner())
    }

    fn context_for_payload<'a>(
        &self,
        payload: &'a LoadExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error> {
        <Self as ConfigureEngineEvm<ExecutionData>>::context_for_payload(self, payload.inner())
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &LoadExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        <Self as ConfigureEngineEvm<ExecutionData>>::tx_iterator_for_payload(self, payload.inner())
    }
}

#[cfg(test)]
mod tests {
    use alloy_rpc_types_engine::PayloadAttributes;
    use reth_payload_primitives::PayloadBuilderAttributes;

    use super::*;

    #[test]
    fn prev_randao_guard() {
        assert!(validate_prev_randao(B256::from(LOAD_PREVRANDAO)).is_ok());
        assert!(validate_prev_randao(B256::ZERO).is_err());
    }

    #[test]
    fn reject_eip7594_sidecars() {
        let payload = empty_load_payload(PayloadId::default())
            .inner
            .with_sidecars(BlobSidecars::Eip7594(Vec::new()));
        let err =
            LoadBuiltPayload::new(payload).try_into() as Result<ExecutionPayloadEnvelopeV3, _>;
        assert!(matches!(err.unwrap_err(), BuiltPayloadConversionError::UnexpectedEip7594Sidecars));
    }

    #[test]
    fn accept_empty_sidecars() {
        let payload = empty_load_payload(PayloadId::default());
        let env: ExecutionPayloadEnvelopeV3 = payload.try_into().unwrap();
        assert_eq!(env.blobs_bundle.blobs.len(), 0);
    }

    #[test]
    fn cap_blob_count() {
        use alloy_eips::eip4844::BlobTransactionSidecar;
        let sidecars = vec![BlobTransactionSidecar::default(); (LOAD_MAX_BLOB_COUNT as usize) + 1];
        let payload = empty_load_payload(PayloadId::default())
            .inner
            .with_sidecars(BlobSidecars::Eip4844(sidecars));
        let err =
            LoadBuiltPayload::new(payload).try_into() as Result<ExecutionPayloadEnvelopeV3, _>;
        assert!(matches!(err.unwrap_err(), BuiltPayloadConversionError::UnexpectedEip4844Sidecars));
    }

    #[test]
    fn builder_attributes_reject_wrong_prev_randao() {
        let rpc_attrs = PayloadAttributes {
            timestamp: 1,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: alloy_primitives::Address::ZERO,
            withdrawals: None,
            parent_beacon_block_root: None,
        };

        let err = LoadPayloadBuilderAttributes::try_new(
            B256::ZERO,
            LoadPayloadAttributes { inner: rpc_attrs },
            3,
        )
        .unwrap_err();
        assert!(err.to_string().contains("prev_randao"));
    }

    #[test]
    fn builder_attributes_accept_constant_prev_randao() {
        let rpc_attrs = PayloadAttributes {
            timestamp: 1,
            prev_randao: B256::from(LOAD_PREVRANDAO),
            suggested_fee_recipient: alloy_primitives::Address::ZERO,
            withdrawals: None,
            parent_beacon_block_root: None,
        };

        let attrs = LoadPayloadBuilderAttributes::try_new(
            B256::ZERO,
            LoadPayloadAttributes { inner: rpc_attrs },
            3,
        )
        .expect("valid attrs");
        assert_eq!(attrs.prev_randao().as_slice(), LOAD_PREVRANDAO);
    }

    #[test]
    fn payload_attributes_reject_non_increasing_timestamp() {
        let rpc_attrs = PayloadAttributes {
            timestamp: 1,
            prev_randao: B256::from(LOAD_PREVRANDAO),
            suggested_fee_recipient: alloy_primitives::Address::ZERO,
            withdrawals: None,
            parent_beacon_block_root: None,
        };

        let attrs = LoadPayloadBuilderAttributes::try_new(
            B256::ZERO,
            LoadPayloadAttributes { inner: rpc_attrs },
            3,
        )
        .expect("valid attrs");
        let err = validate_payload_attributes(1, &attrs).unwrap_err();
        assert!(matches!(err, LoadPayloadAttributesError::InvalidTimestamp(_)));
    }

    #[test]
    fn payload_attributes_accept_future_timestamp() {
        let rpc_attrs = PayloadAttributes {
            timestamp: 2,
            prev_randao: B256::from(LOAD_PREVRANDAO),
            suggested_fee_recipient: alloy_primitives::Address::ZERO,
            withdrawals: None,
            parent_beacon_block_root: None,
        };

        let attrs = LoadPayloadBuilderAttributes::try_new(
            B256::ZERO,
            LoadPayloadAttributes { inner: rpc_attrs },
            3,
        )
        .expect("valid attrs");
        assert!(validate_payload_attributes(1, &attrs).is_ok());
    }
}
