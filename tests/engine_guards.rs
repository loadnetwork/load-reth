//! Engine API ingress guard tests (prev_randao constant enforcement).

mod common;

use std::sync::Arc;

use alloy_primitives::{Address, B256};
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use common::{funded_genesis, load_payload_attributes, test_wallet};
use eyre::Result;
use load_reth::{chainspec::LoadChainSpec, node::LoadNode, LOAD_PREVRANDAO};
use reth_chainspec::EthChainSpec;
use reth_e2e_test_utils::node::NodeTestContext;
use reth_node_api::EngineApiMessageVersion;
use reth_node_builder::NodeBuilder;
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_tasks::TaskManager;

#[tokio::test(flavor = "multi_thread")]
async fn forkchoice_rejects_wrong_prev_randao() -> Result<()> {
    reth_tracing::init_test_tracing();

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

    let handle = node.inner.add_ons_handle.beacon_engine_handle.clone();
    let genesis_hash = chain_spec.genesis_hash();
    let state = ForkchoiceState {
        head_block_hash: genesis_hash,
        safe_block_hash: genesis_hash,
        finalized_block_hash: genesis_hash,
    };

    let attrs = PayloadAttributes {
        timestamp: 1,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };

    let result = handle
        .fork_choice_updated(
            state,
            Some(load_reth::engine::payload::LoadPayloadAttributes { inner: attrs }),
            EngineApiMessageVersion::default(),
        )
        .await;

    assert!(result.is_err(), "engine should reject payload attributes with wrong prev_randao");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn forkchoice_accepts_constant_prev_randao() -> Result<()> {
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

    let handle = node.inner.add_ons_handle.beacon_engine_handle.clone();
    let genesis_hash = chain_spec.genesis_hash();
    let state = ForkchoiceState {
        head_block_hash: genesis_hash,
        safe_block_hash: genesis_hash,
        finalized_block_hash: genesis_hash,
    };

    let attrs = PayloadAttributes {
        timestamp: 1,
        prev_randao: B256::from(LOAD_PREVRANDAO),
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };

    let result = handle
        .fork_choice_updated(
            state,
            Some(load_reth::engine::payload::LoadPayloadAttributes { inner: attrs }),
            EngineApiMessageVersion::default(),
        )
        .await;

    assert!(result.is_ok(), "engine should accept attributes with LOAD_PREVRANDAO");

    Ok(())
}
