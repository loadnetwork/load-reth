//! Load-reth client identity helpers.

use std::sync::OnceLock;

use alloy_rpc_types_engine::ClientVersionV1;
use reth_node_core::version::CLIENT_CODE;

const DEFAULT_NAME: &str = "load-reth";

static CLIENT_VERSION: OnceLock<String> = OnceLock::new();

/// Returns the version string advertised over RPC/p2p.
pub fn load_client_version_string() -> &'static str {
    CLIENT_VERSION.get_or_init(|| {
        let sha = option_env!("VERGEN_GIT_SHA_SHORT").unwrap_or("unknown");
        format!("{DEFAULT_NAME}/v{}-{sha}", env!("CARGO_PKG_VERSION"))
    })
}

/// Returns the Engine API client-identification payload for load-reth.
pub fn load_client_version_entry() -> ClientVersionV1 {
    ClientVersionV1 {
        code: CLIENT_CODE,
        name: DEFAULT_NAME.to_string(),
        version: load_client_version_string().to_string(),
        commit: load_commit(),
    }
}

fn load_commit() -> String {
    option_env!("VERGEN_GIT_SHA_LONG")
        .map(|sha| format!("0x{sha}"))
        .unwrap_or_else(|| "0x0".to_string())
}
