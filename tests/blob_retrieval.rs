//! Ensures engine_getBlobsV1/V2 return stored sidecars for committed blob transactions.
//! Uses a high-stack thread due to blob size (131 KB each).

mod common;

use std::sync::Arc;

use alloy_eips::eip4844::{BlobAndProofV1, BlobAndProofV2};
use alloy_primitives::B256;
use alloy_signer::Signer;
use common::{blob_tx_with_nonce, funded_genesis, load_payload_attributes, test_wallet};
use jsonrpsee::{
    core::client::{ClientT, Error as RpcError},
    rpc_params,
};
use load_reth::{
    chainspec::{LoadChainSpec, LOAD_MAX_BLOB_COUNT},
    node::LoadNode,
};
use reth::chainspec::{EthereumHardfork, ForkCondition};
use reth_chainspec::EthChainSpec;
use reth_e2e_test_utils::node::NodeTestContext;
use reth_ethereum_engine_primitives::BlobSidecars;
use reth_network::{NetworkSyncUpdater, SyncState};
use reth_node_builder::NodeBuilder;
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_payload_primitives::BuiltPayload;
use reth_tasks::TaskManager;

#[test]
fn engine_get_blobs_returns_available_sidecars() {
    // Blob serialization uses large 131 KB buffers; run the test in a dedicated thread with
    // a fat stack plus an 8 MB-per-thread Tokio runtime to avoid stack overflows.
    std::thread::Builder::new()
        .name("blob_retrieval".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(8 * 1024 * 1024)
                .build()
                .expect("tokio runtime");
            match rt.block_on(async move {
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

                let node_handle = NodeBuilder::new(node_config)
                    .testing_node(exec)
                    .node(LoadNode::default())
                    .launch()
                    .await?;

                let mut node =
                    NodeTestContext::new(node_handle.node, load_payload_attributes).await?;

                // Inject multiple blob txs (2 txs × 2 blobs) to ensure multi-entry responses.
                for nonce in 0..2u64 {
                    let tx = blob_tx_with_nonce(chain_id, wallet.clone(), nonce, 2).await?;
                    node.rpc.inject_tx(tx).await?;
                }

                // Build and import a payload so blobs are stored alongside canonical block data.
                let payload = node.new_payload().await?;
                let new_head = payload.block().hash();
                let parent = payload.block().header().parent_hash;
                let block_hash = node.submit_payload(payload.clone()).await?;
                assert_eq!(block_hash, new_head);
                node.update_forkchoice(parent, new_head).await?;

                // Collect the versioned hashes from the built payload's sidecars.
                let versioned_hashes: Vec<B256> = match payload.inner.sidecars() {
                    BlobSidecars::Eip4844(sidecars) => {
                        sidecars.iter().flat_map(|sidecar| sidecar.versioned_hashes()).collect()
                    }
                    _ => Vec::new(),
                };
                assert!(
                    !versioned_hashes.is_empty(),
                    "expected blob transactions to produce versioned hashes"
                );

                // Drive engine_getBlobsV1/V2 through the authenticated Engine RPC client.
                let engine_client = node.inner.engine_http_client();

                let blobs_v1: Vec<Option<BlobAndProofV1>> = ClientT::request(
                    &engine_client,
                    "engine_getBlobsV1",
                    rpc_params![versioned_hashes.clone()],
                )
                .await?;
                assert_eq!(blobs_v1.len(), versioned_hashes.len());
                assert!(
                    blobs_v1.iter().all(|maybe| maybe.is_some()),
                    "all requested blobs should be present in V1 response"
                );

                let blobs_v2: Result<Option<Vec<BlobAndProofV2>>, _> = ClientT::request(
                    &engine_client,
                    "engine_getBlobsV2",
                    rpc_params![versioned_hashes.clone()],
                )
                .await;
                let err = blobs_v2
                    .expect_err("engine_getBlobsV2 should be gated before Osaka (UnsupportedFork)");
                if let RpcError::Call(obj) = err {
                    assert_eq!(obj.code(), -38005, "expected UnsupportedFork error code");
                } else {
                    panic!("unexpected error variant: {err:?}");
                }

                // Empty request should respond immediately with an empty array.
                let empty_v1: Vec<Option<BlobAndProofV1>> = ClientT::request(
                    &engine_client,
                    "engine_getBlobsV1",
                    rpc_params![Vec::<B256>::new()],
                )
                .await?;
                assert!(empty_v1.is_empty(), "empty request should return empty result");

                // Unknown hash should return None entry without erroring.
                let fake_hash = B256::random();
                let missing_v1: Vec<Option<BlobAndProofV1>> = ClientT::request(
                    &engine_client,
                    "engine_getBlobsV1",
                    rpc_params![vec![fake_hash]],
                )
                .await?;
                assert_eq!(missing_v1.len(), 1);
                assert!(missing_v1[0].is_none(), "unknown hash should map to None");

                Ok::<_, eyre::Report>(())
            }) {
                Ok(()) => {}
                Err(err) => panic!("blob retrieval test failed: {err:?}"),
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_get_blobs_rejects_requests_over_cap() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let wallet = test_wallet();
    let genesis = funded_genesis(&[wallet.address()]);
    let chain_spec = Arc::new(LoadChainSpec::from_genesis(genesis)?);

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let node_handle =
        NodeBuilder::new(node_config).testing_node(exec).node(LoadNode::default()).launch().await?;

    let node = NodeTestContext::new(node_handle.node, load_payload_attributes).await?;
    let engine_client = node.inner.engine_http_client();

    let oversized = vec![B256::ZERO; (LOAD_MAX_BLOB_COUNT + 1) as usize];
    let res: Result<Vec<Option<BlobAndProofV1>>, _> =
        ClientT::request(&engine_client, "engine_getBlobsV1", rpc_params![oversized]).await;

    let err = res.expect_err("engine_getBlobsV1 should reject requests over the Load cap");
    if let RpcError::Call(obj) = err {
        // Reth returns -38004 "Too large request" for over-cap blob queries.
        assert_eq!(obj.code(), -38004, "expected RequestTooLarge error code");
    } else {
        panic!("unexpected error variant: {err:?}");
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_get_blobs_v2_rejected_before_osaka() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let wallet = test_wallet();
    let genesis = funded_genesis(&[wallet.address()]);
    let mut spec = LoadChainSpec::from_genesis(genesis)?;
    spec.inner.hardforks.insert(EthereumHardfork::Osaka, ForkCondition::Timestamp(u64::MAX));
    let chain_spec = Arc::new(spec);

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let node_handle =
        NodeBuilder::new(node_config).testing_node(exec).node(LoadNode::default()).launch().await?;

    let node = NodeTestContext::new(node_handle.node, load_payload_attributes).await?;
    let engine_client = node.inner.engine_http_client();

    let res: Result<Option<Vec<BlobAndProofV2>>, _> =
        ClientT::request(&engine_client, "engine_getBlobsV2", rpc_params![Vec::<B256>::new()])
            .await;

    let err = res.expect_err("engine_getBlobsV2 should be rejected before Osaka activation");
    if let RpcError::Call(obj) = err {
        // -38005: UnsupportedFork (per Engine API spec)
        assert_eq!(obj.code(), -38005, "expected UnsupportedFork error code");
    } else {
        panic!("unexpected error variant: {err:?}");
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_get_blobs_v3_rejected_before_osaka() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let wallet = test_wallet();
    let genesis = funded_genesis(&[wallet.address()]);
    let mut spec = LoadChainSpec::from_genesis(genesis)?;
    spec.inner.hardforks.insert(EthereumHardfork::Osaka, ForkCondition::Timestamp(u64::MAX));
    let chain_spec = Arc::new(spec);

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let node_handle =
        NodeBuilder::new(node_config).testing_node(exec).node(LoadNode::default()).launch().await?;

    let node = NodeTestContext::new(node_handle.node, load_payload_attributes).await?;
    let engine_client = node.inner.engine_http_client();

    let res: Result<Option<Vec<Option<BlobAndProofV2>>>, _> =
        ClientT::request(&engine_client, "engine_getBlobsV3", rpc_params![Vec::<B256>::new()])
            .await;

    let err = res.expect_err("engine_getBlobsV3 should be rejected before Osaka activation");
    if let RpcError::Call(obj) = err {
        // -38005: UnsupportedFork (per Engine API spec)
        assert_eq!(obj.code(), -38005, "expected UnsupportedFork error code");
    } else {
        panic!("unexpected error variant: {err:?}");
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_get_blobs_v3_rejects_requests_over_cap() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let wallet = test_wallet();
    let genesis = funded_genesis(&[wallet.address()]);
    let mut spec = LoadChainSpec::from_genesis(genesis)?;
    spec.inner.hardforks.insert(EthereumHardfork::Osaka, ForkCondition::Timestamp(0));
    let chain_spec = Arc::new(spec);

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let node_handle =
        NodeBuilder::new(node_config).testing_node(exec).node(LoadNode::default()).launch().await?;

    let node = NodeTestContext::new(node_handle.node, load_payload_attributes).await?;
    let engine_client = node.inner.engine_http_client();

    let oversized = vec![B256::ZERO; (LOAD_MAX_BLOB_COUNT + 1) as usize];
    let res: Result<Option<Vec<Option<BlobAndProofV2>>>, _> =
        ClientT::request(&engine_client, "engine_getBlobsV3", rpc_params![oversized]).await;

    let err = res.expect_err("engine_getBlobsV3 should reject requests over the Load cap");
    if let RpcError::Call(obj) = err {
        // Reth returns -38004 "Too large request" for over-cap blob queries.
        assert_eq!(obj.code(), -38004, "expected RequestTooLarge error code");
    } else {
        panic!("unexpected error variant: {err:?}");
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_get_blobs_v3_returns_null_when_syncing() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let wallet = test_wallet();
    let genesis = funded_genesis(&[wallet.address()]);
    let mut spec = LoadChainSpec::from_genesis(genesis)?;
    spec.inner.hardforks.insert(EthereumHardfork::Osaka, ForkCondition::Timestamp(0));
    let chain_spec = Arc::new(spec);

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let node_handle =
        NodeBuilder::new(node_config).testing_node(exec).node(LoadNode::default()).launch().await?;

    let node = NodeTestContext::new(node_handle.node, load_payload_attributes).await?;
    node.inner.network.update_sync_state(SyncState::Syncing);

    let engine_client = node.inner.engine_http_client();
    let res: Option<Vec<Option<BlobAndProofV2>>> = ClientT::request(
        &engine_client,
        "engine_getBlobsV3",
        rpc_params![vec![B256::random()]],
    )
    .await?;

    assert!(res.is_none(), "expected null response while syncing");

    Ok(())
}
