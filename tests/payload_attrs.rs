//! Prague gating regression test (ensures proposer metadata isn't accepted before activation).

mod common;

use std::{convert::TryInto, sync::Arc};

use alloy_eips::eip7685::{Requests, RequestsOrHash};
use alloy_primitives::{Address, B256};
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV3, PayloadAttributes, PayloadStatusEnum};
use alloy_signer::Signer;
use common::{blob_tx_with_nonce, funded_genesis, load_payload_attributes, test_wallet};
use eyre::Result;
use load_reth::{
    chainspec::LoadChainSpec,
    engine::payload::{LoadExecutionData, LoadPayloadAttributes, LoadPayloadBuilderAttributes},
    node::LoadNode,
    LOAD_PREVRANDAO,
};
use reth::chainspec::{EthereumHardfork, ForkCondition};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_e2e_test_utils::node::NodeTestContext;
use reth_ethereum_engine_primitives::BlobSidecars;
use reth_node_builder::NodeBuilder;
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_payload_primitives::PayloadBuilderAttributes;
use reth_tasks::TaskManager;

#[tokio::test(flavor = "multi_thread")]
async fn prague_requests_rejected_before_activation() -> Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let mut wallet = test_wallet();
    let genesis = funded_genesis(&[wallet.address()]);
    let mut spec = LoadChainSpec::from_genesis(genesis)?;
    spec.inner.hardforks.insert(EthereumHardfork::Prague, ForkCondition::Timestamp(1_000));
    assert!(!spec.is_prague_active_at_timestamp(1), "test requires Prague to be inactive");

    let chain_spec = Arc::new(spec);
    let chain_id = chain_spec.chain().id();
    wallet = wallet.with_chain_id(Some(chain_id));

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let node_handle =
        NodeBuilder::new(node_config).testing_node(exec).node(LoadNode::default()).launch().await?;

    let mut node = NodeTestContext::new(node_handle.node, load_payload_attributes).await?;

    let tx = blob_tx_with_nonce(chain_id, wallet.clone(), 0, 1).await?;
    node.rpc.inject_tx(tx).await?;

    let payload = node.new_payload().await?;
    let envelope: ExecutionPayloadEnvelopeV3 =
        payload.clone().try_into().expect("payload should convert to V3");

    let load_payload = LoadExecutionData::v4(
        envelope.execution_payload,
        Vec::new(),
        B256::ZERO,
        RequestsOrHash::Requests(Requests::default()),
    );

    let handle = node.inner.add_ons_handle.beacon_engine_handle.clone();
    let status = handle.new_payload(load_payload).await?;
    assert!(
        matches!(status.status, PayloadStatusEnum::Invalid { .. }),
        "engine_newPayload must reject Prague requests before activation (got {:?})",
        status.status
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prague_requests_accepted_after_activation() -> Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let mut wallet = test_wallet();
    let genesis = funded_genesis(&[wallet.address()]);
    let mut spec = LoadChainSpec::from_genesis(genesis)?;
    spec.inner.hardforks.insert(EthereumHardfork::Prague, ForkCondition::Timestamp(0));
    assert!(spec.is_prague_active_at_timestamp(1), "test requires Prague to be active");

    let chain_spec = Arc::new(spec);
    let chain_id = chain_spec.chain().id();
    wallet = wallet.with_chain_id(Some(chain_id));

    let parent_beacon_block_root = B256::random();
    let attrs_gen = {
        let pbbr = parent_beacon_block_root;
        move |timestamp| {
            let rpc_attrs = PayloadAttributes {
                timestamp,
                prev_randao: B256::from(LOAD_PREVRANDAO),
                suggested_fee_recipient: Address::ZERO,
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(pbbr),
            };
            LoadPayloadBuilderAttributes::try_new(
                B256::ZERO,
                LoadPayloadAttributes { inner: rpc_attrs },
                3,
            )
            .expect("valid payload attributes")
        }
    };

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let node_handle =
        NodeBuilder::new(node_config).testing_node(exec).node(LoadNode::default()).launch().await?;

    let mut node = NodeTestContext::new(node_handle.node, attrs_gen).await?;

    // Stage a blob tx so the payload carries versioned hashes and a sidecar.
    let tx = blob_tx_with_nonce(chain_id, wallet.clone(), 0, 1).await?;
    node.rpc.inject_tx(tx).await?;

    let payload = node.new_payload().await?;

    let versioned_hashes: Vec<B256> = match payload.inner.sidecars() {
        BlobSidecars::Eip4844(sidecars) => {
            sidecars.iter().flat_map(|sidecar| sidecar.versioned_hashes()).collect()
        }
        _ => Vec::new(),
    };
    assert!(
        !versioned_hashes.is_empty(),
        "Prague acceptance test requires blob versioned hashes in the sidecar"
    );

    let envelope: ExecutionPayloadEnvelopeV3 =
        payload.clone().try_into().expect("payload should convert to V3");

    // Load Network does not deploy the Prague execution-request system contracts, so
    // execution always produces EMPTY_REQUESTS_HASH. The Engine API must therefore
    // accept empty request lists after Prague activation.
    let requests = Requests::default();
    let load_payload = LoadExecutionData::v4(
        envelope.execution_payload,
        versioned_hashes,
        parent_beacon_block_root,
        RequestsOrHash::Requests(requests),
    );

    let handle = node.inner.add_ons_handle.beacon_engine_handle.clone();
    let status = handle.new_payload(load_payload).await?;
    assert!(
        matches!(status.status, PayloadStatusEnum::Valid { .. }),
        "engine_newPayload must accept Prague requests after activation (got {:?})",
        status.status
    );

    Ok(())
}
