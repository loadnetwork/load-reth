//! FCU→getPayload positive scenarios ensuring blob-heavy payloads are built and capped correctly.

mod common;

use std::{env, sync::Arc};

use alloy_signer::Signer;
use common::{blob_tx_with_nonce, funded_genesis, load_payload_attributes, test_wallet};
use load_reth::{
    chainspec::{LoadChainSpec, LOAD_MAX_BLOB_COUNT},
    node::LoadNode,
};
use reth_chainspec::EthChainSpec;
use reth_e2e_test_utils::node::NodeTestContext;
use reth_node_builder::NodeBuilder;
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_payload_primitives::BuiltPayload;
use reth_tasks::TaskManager;
use rstest::rstest;

#[rstest]
#[case(8, 1, 8)]
#[case(12, 2, 24)]
#[tokio::test(flavor = "multi_thread")]
async fn payload_includes_blob_sidecars(
    #[case] txs: usize,
    #[case] blobs_per_tx: usize,
    #[case] expected_blobs: usize,
) -> eyre::Result<()> {
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

    for nonce in 0..txs {
        let tx = blob_tx_with_nonce(chain_id, wallet.clone(), nonce as u64, blobs_per_tx).await?;
        node.rpc.inject_tx(tx).await?;
    }

    let payload = node.new_payload().await?;
    let tx_count = payload.block().body().transactions().count();
    assert_eq!(tx_count, txs, "all transactions should be included");

    let (sidecar_count, total_blobs) = match payload.inner.sidecars() {
        reth_ethereum_engine_primitives::BlobSidecars::Eip4844(sidecars) => {
            let blobs = sidecars.iter().map(|s| s.blobs.len()).sum();
            (sidecars.len(), blobs)
        }
        _ => (0, 0),
    };
    assert_eq!(sidecar_count, txs, "one sidecar per tx expected");
    assert!(
        total_blobs >= expected_blobs && total_blobs <= LOAD_MAX_BLOB_COUNT as usize,
        "blob count should be at least expected ({expected_blobs}) and within cap (got {total_blobs})"
    );

    let new_head = payload.block().hash();
    let parent = payload.block().header().parent_hash;
    let block_hash = node.submit_payload(payload).await?;
    assert_eq!(block_hash, new_head);
    node.update_forkchoice(parent, new_head).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn payload_handles_near_full_blob_cap() -> eyre::Result<()> {
    // Heavy: requires ~2 minutes and high memory. Opt-in via LOAD_BLOB_STRESS=1.
    if env::var("LOAD_BLOB_STRESS").is_err() {
        eprintln!("skipping near-cap blob stress; set LOAD_BLOB_STRESS=1 to run");
        return Ok(());
    }

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
    let chain_id = chain_spec.chain().id();

    // 32 txs × 32 blobs = 1024 (Load cap)
    let txs = 32usize;
    let blobs_per_tx = 32usize;
    for nonce in 0..txs {
        let tx = blob_tx_with_nonce(chain_id, wallet.clone(), nonce as u64, blobs_per_tx).await?;
        node.rpc.inject_tx(tx).await?;
    }

    let payload = node.new_payload().await?;
    let total_blobs = match payload.inner.sidecars() {
        reth_ethereum_engine_primitives::BlobSidecars::Eip4844(sidecars) => {
            sidecars.iter().map(|s| s.blobs.len()).sum()
        }
        _ => 0,
    };

    assert_eq!(total_blobs, LOAD_MAX_BLOB_COUNT as usize, "cap should apply (got {total_blobs})");

    let new_head = payload.block().hash();
    let parent = payload.block().header().parent_hash;
    let block_hash = node.submit_payload(payload).await?;
    assert_eq!(block_hash, new_head);
    node.update_forkchoice(parent, new_head).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn payload_truncates_blobs_over_cap() -> eyre::Result<()> {
    // Heavy: same stress conditions as the near-cap test; opt-in via LOAD_BLOB_STRESS=1.
    if env::var("LOAD_BLOB_STRESS").is_err() {
        eprintln!("skipping over-cap blob stress; set LOAD_BLOB_STRESS=1 to run");
        return Ok(());
    }

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
    let chain_id = chain_spec.chain().id();

    // 33 txs × 32 blobs = 1056 submitted; builder must cap at 1024.
    let txs = 33usize;
    let blobs_per_tx = 32usize;
    for nonce in 0..txs {
        let tx = blob_tx_with_nonce(chain_id, wallet.clone(), nonce as u64, blobs_per_tx).await?;
        node.rpc.inject_tx(tx).await?;
    }

    let payload = node.new_payload().await?;
    let total_blobs = match payload.inner.sidecars() {
        reth_ethereum_engine_primitives::BlobSidecars::Eip4844(sidecars) => {
            sidecars.iter().map(|s| s.blobs.len()).sum()
        }
        _ => 0,
    };

    assert_eq!(
        total_blobs, LOAD_MAX_BLOB_COUNT as usize,
        "payload should enforce the {}-blob cap even when more are available",
        LOAD_MAX_BLOB_COUNT
    );

    let new_head = payload.block().hash();
    let parent = payload.block().header().parent_hash;
    let block_hash = node.submit_payload(payload).await?;
    assert_eq!(block_hash, new_head);
    node.update_forkchoice(parent, new_head).await?;
    Ok(())
}
