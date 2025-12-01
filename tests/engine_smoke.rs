//! Minimal smoke test ensuring Engine RPC serves capabilities and enforces
//! Load-specific guards (fixed PREVRANDAO).

use std::sync::Arc;

use alloy_genesis::Genesis;
use alloy_primitives::{Address, B256};
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use eyre::Context;
use load_reth::{engine::payload::LoadPayloadAttributes, node::LoadNode, LoadChainSpec};
use reth_node_builder::NodeBuilder;
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_rpc_api::clients::EngineApiClient;
use reth_tasks::TaskManager;

const DEV_GENESIS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/etc/load-dev-genesis.json");

#[tokio::test(flavor = "multi_thread")]
async fn engine_rpc_smoke_serves_caps_and_prev_randao_guard() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let genesis: Genesis = serde_json::from_str(
        &std::fs::read_to_string(DEV_GENESIS_PATH)
            .context("reading bundled load-dev genesis json")?,
    )?;
    let chain_spec = Arc::new(LoadChainSpec::from_genesis(genesis)?);

    let node_config = NodeConfig::new(chain_spec)
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let node_handle =
        NodeBuilder::new(node_config).testing_node(exec).node(LoadNode::default()).launch().await?;

    let reth_node = node_handle.node;
    let engine = reth_node.engine_http_client();

    // 1. Capabilities should advertise Load’s blob + prev_randao strings.
    let exchange = engine
        .exchange_capabilities(vec!["eth".to_string()])
        .await
        .context("engine_exchangeCapabilities failed")?;
    let has_load_caps = exchange.contains(&"load.blobs.1024".to_string()) &&
        exchange.contains(&"load.prev_randao.0x01".to_string());
    assert!(has_load_caps, "capabilities should include Load flags: {exchange:?}");

    // 2. forkchoiceUpdatedV3 should reject payload attributes with wrong prev_randao (0x00).
    let forkchoice = ForkchoiceState::default();
    let attrs = LoadPayloadAttributes {
        inner: PayloadAttributes {
            timestamp: 1,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: Address::ZERO,
            withdrawals: Some(vec![]),
            parent_beacon_block_root: Some(B256::ZERO),
        },
    };

    let response = engine.fork_choice_updated_v3(forkchoice, Some(attrs)).await;
    assert!(response.is_err(), "forkchoiceUpdatedV3 should reject wrong prev_randao");

    drop(node_handle.node_exit_future);
    Ok(())
}
