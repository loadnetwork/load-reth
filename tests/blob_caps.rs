//! Negative scenarios validating blob caps (per-tx and per-payload versioned hashes) at ingress.

mod common;

use std::{convert::TryInto, sync::Arc};

use alloy_primitives::B256;
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV3, PayloadStatusEnum};
use alloy_signer::Signer;
use common::{blob_tx_with_nonce, funded_genesis, load_payload_attributes, test_wallet};
use load_reth::{
    chainspec::{LoadChainSpec, LOAD_MAX_BLOBS_PER_TX, LOAD_MAX_BLOB_COUNT},
    engine::payload::LoadExecutionData,
    node::LoadNode,
};
use reth_chainspec::EthChainSpec;
use reth_e2e_test_utils::node::NodeTestContext;
use reth_node_builder::NodeBuilder;
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_tasks::TaskManager;

#[tokio::test(flavor = "multi_thread")]
async fn tx_with_too_many_blobs_is_rejected_at_ingress() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let mut wallet = test_wallet();
    let genesis = funded_genesis(&[wallet.address()]);
    let chain_spec = Arc::new(LoadChainSpec::from_genesis(genesis)?);
    let chain_id = chain_spec.chain().id();
    wallet = wallet.with_chain_id(Some(chain_id));

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let node_handle =
        NodeBuilder::new(node_config).testing_node(exec).node(LoadNode::default()).launch().await?;

    let node = NodeTestContext::new(node_handle.node, load_payload_attributes).await?;

    // One transaction carrying more than the per-tx cap should be rejected by the pool validator.
    let tx = blob_tx_with_nonce(chain_id, wallet.clone(), 0, (LOAD_MAX_BLOBS_PER_TX + 1) as usize)
        .await?;

    let res = node.rpc.inject_tx(tx).await;
    assert!(res.is_err(), "tx with >{} blobs should be rejected", LOAD_MAX_BLOBS_PER_TX);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn payload_with_too_many_versioned_hashes_is_rejected() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let mut wallet = test_wallet();
    let genesis = funded_genesis(&[wallet.address()]);
    let chain_spec = Arc::new(LoadChainSpec::from_genesis(genesis)?);
    let chain_id = chain_spec.chain().id();
    wallet = wallet.with_chain_id(Some(chain_id));

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let node_handle =
        NodeBuilder::new(node_config).testing_node(exec).node(LoadNode::default()).launch().await?;

    let mut node = NodeTestContext::new(node_handle.node, load_payload_attributes).await?;

    // Stage at least one blob tx so the payload builder resolves quickly.
    let tx = blob_tx_with_nonce(chain_id, wallet.clone(), 0, 1).await?;
    node.rpc.inject_tx(tx).await?;

    // Build a payload and then submit it with too many blob hashes.
    let payload = node.new_payload().await?;
    let envelope: ExecutionPayloadEnvelopeV3 =
        payload.clone().try_into().expect("payload should convert to v3");
    let execution_payload = envelope.execution_payload;

    let versioned_hashes = vec![B256::ZERO; (LOAD_MAX_BLOB_COUNT + 1) as usize];
    let engine_payload = LoadExecutionData::v3(execution_payload, versioned_hashes, B256::ZERO);

    let handle = node.inner.add_ons_handle.beacon_engine_handle.clone();
    let status = handle.new_payload(engine_payload).await?;
    assert!(
        matches!(status.status, PayloadStatusEnum::Invalid { .. }),
        "engine_newPayload must report invalid status for >{} blob hashes (got {:?})",
        LOAD_MAX_BLOB_COUNT,
        status.status
    );

    Ok(())
}
