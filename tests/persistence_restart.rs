//! Persistence guard for 1-slot finality.
//!
//! Ensures canonical blocks are persisted to disk immediately (persistence_threshold=0),
//! so a crash/restart cannot lose finalized data.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy_signer::Signer;
use common::{funded_genesis, load_payload_attributes, test_wallet};
use eyre::{bail, Result};
use load_reth::{chainspec::LoadChainSpec, node::LoadNode};
use reth_db::init_db;
use reth_chainspec::EthChainSpec;
use reth_e2e_test_utils::{node::NodeTestContext, transaction::TransactionTestContext};
use reth_node_builder::NodeBuilder;
use reth_node_core::{
    args::{DatadirArgs, DefaultEngineValues, RpcServerArgs},
    node_config::NodeConfig,
};
use reth_provider::BlockNumReader;
use reth_tasks::TaskManager;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread")]
async fn persistence_threshold_zero_survives_restart() -> Result<()> {
    reth_tracing::init_test_tracing();

    // Ensure the engine defaults match production (persistence_threshold=0).
    let _ = DefaultEngineValues::default().with_persistence_threshold(0).try_init();

    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let mut wallet = test_wallet();
    let genesis = funded_genesis(&[wallet.address()]);
    let chain_spec = Arc::new(LoadChainSpec::from_genesis(genesis)?);
    let chain_id = chain_spec.chain().id();
    wallet = wallet.with_chain_id(Some(chain_id));

    let temp_dir = TempDir::new()?;
    let datadir_args =
        DatadirArgs { datadir: temp_dir.path().to_path_buf().into(), ..Default::default() };

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_datadir_args(datadir_args.clone())
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    assert_eq!(
        node_config.engine.persistence_threshold, 0,
        "persistence_threshold must be 0 for Load 1-slot finality"
    );

    let data_dir = node_config.datadir();
    let db_path = data_dir.db();
    let db_deadline = Instant::now() + Duration::from_secs(2);
    let db = loop {
        match init_db(&db_path, node_config.db.database_args()) {
            Ok(db) => break Arc::new(db),
            Err(err) => {
                if Instant::now() > db_deadline {
                    return Err(err);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };

    let node_handle = NodeBuilder::new(node_config)
        .with_database(db.clone())
        .with_launch_context(exec.clone())
        .node(LoadNode::default())
        .launch()
        .await?;

    let mut node = NodeTestContext::new(node_handle.node, load_payload_attributes).await?;

    let raw_tx = TransactionTestContext::transfer_tx_bytes(chain_id, wallet).await;
    node.rpc.inject_tx(raw_tx).await?;

    node.advance_block().await?;

    let best = node.inner.provider.best_block_number()?;
    assert_eq!(best, 1, "expected a single canonical block");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let persisted = node.inner.provider.last_block_number()?;
        if persisted >= best {
            break;
        }
        if Instant::now() > deadline {
            bail!(
                "expected block {} to be persisted immediately (last_block_number={})",
                best,
                persisted
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    if let Some(done_rx) = node.inner.add_ons_handle.engine_shutdown.shutdown() {
        tokio::time::timeout(Duration::from_secs(2), done_rx)
            .await
            .expect("engine shutdown timed out")
            .expect("engine shutdown channel closed");
    }

    drop(node);
    drop(db);

    let shutdown_ok = tasks.graceful_shutdown_with_timeout(Duration::from_secs(2));
    assert!(shutdown_ok, "task shutdown timed out");

    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let node_config = NodeConfig::new(chain_spec.clone())
        .with_datadir_args(datadir_args)
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

    let data_dir = node_config.datadir();
    let db_path = data_dir.db();
    let db_deadline = Instant::now() + Duration::from_secs(2);
    let db = loop {
        match init_db(&db_path, node_config.db.database_args()) {
            Ok(db) => break Arc::new(db),
            Err(err) => {
                if Instant::now() > db_deadline {
                    return Err(err);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };

    let node_handle = NodeBuilder::new(node_config)
        .with_database(db)
        .with_launch_context(exec)
        .node(LoadNode::default())
        .launch()
        .await?;

    let node = NodeTestContext::new(node_handle.node, load_payload_attributes).await?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let persisted = node.inner.provider.last_block_number()?;
        if persisted >= best {
            break;
        }
        if Instant::now() > deadline {
            bail!(
                "restart should see persisted canonical block (last_block_number={})",
                persisted
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    Ok(())
}
