//! load-reth: Load Network execution client built on the reth SDK.
//!
//! Load-reth is an Ethereum-compatible execution client with enhanced data availability:
//! - **Blob support**: Up to 1024 blobs per block (vs Ethereum's 6)
//! - **Target throughput**: 512 blobs per block (~67 MB/block)
//! - **Cancun at genesis**: EIP-4844 blob transactions from block 0
//! - **Prague scheduled**: Proposer metadata support via genesis config
//!
//! ## Architecture
//!
//! Load-reth follows the Ethereum CL/EL split pattern, communicating with Ultramarine
//! (the consensus layer) via Engine API v3. The design reuses standard Ethereum
//! primitives where possible and only customizes:
//! - Chain specification with custom blob parameters
//! - Transaction pool with larger blob cache (32,768 blobs ≈ 4.3 GB)
//! - Passive consensus validation (Ultramarine drives finality)
//!
//! ## Key Differences from Ethereum
//!
//! | Parameter | Ethereum | Load Network |
//! |-----------|----------|--------------|
//! | Max blobs/block | 6 | 1024 |
//! | Target blobs/block | 3 | 512 |
//! | Blob cache size | ~384 blobs | ~32,768 blobs |
//! | PREVRANDAO | Random | Fixed (0x01) |
//!
//! ## Usage
//!
//! ```bash
//! # Initialize with Load genesis
//! load-reth init --chain etc/load-dev-genesis.json
//!
//! # Run node
//! load-reth node \
//!   --chain etc/load-dev-genesis.json \
//!   --http \
//!   --authrpc.jwtsecret /path/to/jwt.hex
//! ```

// Core modules
pub mod chainspec;
pub mod consensus;
pub mod engine;
pub mod evm;
pub mod metrics;
pub mod node;
pub mod pool;
pub mod primitives;
pub mod rpc;
pub mod transaction;
pub mod version;

// Re-export key types
pub use chainspec::{LoadChainSpec, LoadChainSpecParser};
pub use engine::{
    payload::{
        empty_load_payload, validate_prev_randao as validate_payload_prev_randao, LoadBuiltPayload,
        LoadEngineTypes,
    },
    LoadPayloadServiceBuilder,
};
pub use evm::LoadEvmConfig;
pub use node::LoadNode;
pub use pool::LoadPoolBuilder;
pub use primitives::LoadPrimitives;
pub use transaction::{SignedTransaction as LoadSignedTransaction, Transaction as LoadTransaction};

/// Canonical PREVRANDAO value for Load Network (constant 0x01).
///
/// Load Network fixes PREVRANDAO to the constant `0x01` (Arbitrum-style) to
/// explicitly signal that applications must not rely on it for entropy.
///
/// ## Rationale
///
/// - **Explicitly non-random**: Prevents dApps from misusing it as a source of randomness
/// - **Identity property**: `1 × x = x` makes accidental arithmetic safe
/// - **Battle-tested**: Mirrors Arbitrum (0x01) and zkSync (fixed constant) patterns
/// - **Design doc reference**: See `load-el-design/load-reth-design.md` §2
///
/// ## Engine API Contract
///
/// - Ultramarine supplies `prev_randao = 0x01` in every `engine_forkchoiceUpdatedV3` call
/// - Load-reth validates this value and rejects non-matching payloads
/// - Consensus enforces the constant; applications must use VRF/oracles for randomness
pub const LOAD_PREVRANDAO: [u8; 32] = {
    let mut bytes = [0u8; 32];
    bytes[31] = 1;
    bytes
};

#[cfg(test)]
mod tests {
    use alloy_primitives::B256;

    use super::{engine::payload::validate_prev_randao, LOAD_PREVRANDAO};

    #[test]
    fn prev_randao_is_constant_one() {
        let mut expected = [0u8; 32];
        expected[31] = 1;
        assert_eq!(LOAD_PREVRANDAO, expected);
    }

    #[test]
    fn validate_prev_randao_accepts_constant() {
        let ok = validate_prev_randao(B256::from(LOAD_PREVRANDAO));
        assert!(ok.is_ok());
    }

    #[test]
    fn validate_prev_randao_rejects_other_values() {
        let err = validate_prev_randao(B256::ZERO).unwrap_err();
        assert!(err.contains("prev_randao"));
    }
}
