//! Load Network chain specification with custom blob parameters.
//!
//! The design activates Shanghai, Cancun, and Prague/Pectra at timestamp 0 and
//! immediately raises blob limits/pricing to Load constants (1024 max blobs,
//! Pectra update fraction). We wrap `reth`'s `ChainSpec` to own those knobs and
//! keep the rest of the codebase pointed at `LoadChainSpec`.

use std::sync::Arc;

use alloy_eips::{
    eip2124::{ForkFilter, ForkId, Head},
    eip7840::BlobParams,
    eip7892::BlobScheduleBlobParams,
};
use alloy_genesis::Genesis;
use alloy_primitives::U256;
use derive_more::{Constructor, Into};
use eyre::Context;
use reth::chainspec::{Chain, EthereumHardforks, ForkCondition, Hardfork};
use reth_chainspec::{ChainSpec, EthChainSpec, Hardforks};
use reth_cli::chainspec::ChainSpecParser;
use serde_json;
use tracing::{debug, info};

/// Load Network blob parameters (design doc §4.2)
pub const LOAD_MAX_BLOB_COUNT: u64 = 1024;
pub const LOAD_TARGET_BLOB_COUNT: u64 = 512;
pub const LOAD_MAX_BLOBS_PER_TX: u64 = 32;
/// Pectra (EIP-7691) blob pricing update fraction (more responsive than Cancun).
pub const LOAD_BLOB_UPDATE_FRACTION: u128 = 5_007_716;

/// Load chain specification wrapping reth's `ChainSpec`.
#[derive(Debug, Clone, Into, Constructor, PartialEq, Eq)]
pub struct LoadChainSpec {
    /// The underlying reth chain specification.
    pub inner: ChainSpec,
}

impl LoadChainSpec {
    /// Create Load chain spec from genesis configuration.
    pub fn from_genesis(mut genesis: Genesis) -> eyre::Result<Self> {
        info!(chain_id = genesis.config.chain_id, "Creating Load chain spec from genesis");

        // Validate Load-specific requirements and normalize pre-Cancun forks.
        Self::validate_genesis(&mut genesis).context("Genesis validation failed")?;

        // Build the inner chain spec from the normalized genesis.
        let mut inner: ChainSpec = genesis.clone().into();
        inner.chain = Chain::from_id_unchecked(genesis.config.chain_id);

        // Override blob params for Cancun/Prague with Load limits.
        let load_blob_params = BlobParams {
            target_blob_count: LOAD_TARGET_BLOB_COUNT,
            max_blob_count: LOAD_MAX_BLOB_COUNT,
            max_blobs_per_tx: LOAD_MAX_BLOBS_PER_TX,
            // Use Pectra (EIP-7691) pricing dynamics from genesis.
            update_fraction: LOAD_BLOB_UPDATE_FRACTION,
            min_blob_fee: BlobParams::cancun().min_blob_fee,
            blob_base_cost: BlobParams::cancun().blob_base_cost,
        };

        inner.blob_params = BlobScheduleBlobParams {
            cancun: load_blob_params,
            prague: load_blob_params,
            osaka: BlobParams::osaka(),
            scheduled: Default::default(),
        };

        info!(
            chain_id = inner.chain.id(),
            genesis_hash = ?inner.genesis_hash(),
            "Load chain spec created successfully"
        );

        Ok(Self { inner })
    }

    /// Validate genesis configuration meets Load Network requirements and mutates defaults for
    /// pre-Cancun forks to activate at genesis.
    fn validate_genesis(genesis: &mut Genesis) -> eyre::Result<()> {
        if genesis.config.cancun_time != Some(0) {
            eyre::bail!(
                "Load Network requires Cancun hardfork at genesis (cancunTime = 0). Got: {:?}",
                genesis.config.cancun_time
            );
        }

        match genesis.config.terminal_total_difficulty {
            Some(ttd) if ttd != U256::ZERO => {
                eyre::bail!(
                    "Load Network PoS mode requires terminalTotalDifficulty = 0. Got: {}",
                    ttd
                );
            }
            None => {
                genesis.config.terminal_total_difficulty = Some(U256::ZERO);
            }
            _ => {}
        }

        // Merge-at-genesis guardrails.
        if !genesis.config.terminal_total_difficulty_passed {
            genesis.config.terminal_total_difficulty_passed = true;
        }
        if let Some(merge_block) = genesis.config.merge_netsplit_block {
            if merge_block != 0 {
                eyre::bail!(
                    "Load Network requires merge_netsplit_block = 0 (merge at genesis). Got: {merge_block}"
                );
            }
        } else {
            genesis.config.merge_netsplit_block = Some(0);
        }

        // Ensure all pre-Cancun forks are at block 0 if provided (match design doc defaults).
        let pre_cancun_forks = [
            &mut genesis.config.homestead_block,
            &mut genesis.config.dao_fork_block,
            &mut genesis.config.eip150_block,
            &mut genesis.config.eip155_block,
            &mut genesis.config.eip158_block,
            &mut genesis.config.byzantium_block,
            &mut genesis.config.constantinople_block,
            &mut genesis.config.petersburg_block,
            &mut genesis.config.istanbul_block,
            &mut genesis.config.muir_glacier_block,
            &mut genesis.config.berlin_block,
            &mut genesis.config.london_block,
            &mut genesis.config.arrow_glacier_block,
            &mut genesis.config.gray_glacier_block,
        ];

        for fork in pre_cancun_forks {
            if let Some(block) = fork {
                if *block != 0 {
                    eyre::bail!("Load Network requires pre-Cancun forks at block 0 (got {block})");
                }
            } else {
                *fork = Some(0);
            }
        }

        // Ensure Shanghai/Cancun/Prague are active at genesis.
        if let Some(time) = genesis.config.shanghai_time {
            if time != 0 {
                eyre::bail!("Load Network requires Shanghai hardfork at genesis (shanghaiTime = 0). Got: {time}");
            }
        } else {
            genesis.config.shanghai_time = Some(0);
        }

        if let Some(time) = genesis.config.prague_time {
            if time != 0 {
                eyre::bail!("Load Network requires Prague hardfork at genesis (pragueTime = 0). Got: {time}");
            }
        } else {
            genesis.config.prague_time = Some(0);
        }

        debug!("Genesis validation passed");
        Ok(())
    }
}

impl Default for LoadChainSpec {
    fn default() -> Self {
        // Default to a minimal dev genesis (Cancun at 0, chain ID 16383)
        let mut genesis = Genesis::default();
        genesis.config.chain_id = 16_383;
        genesis.config.terminal_total_difficulty = Some(U256::ZERO);
        genesis.config.terminal_total_difficulty_passed = true;
        genesis.config.shanghai_time = Some(0);
        genesis.config.cancun_time = Some(0);
        genesis.config.prague_time = Some(0);
        genesis.config.merge_netsplit_block = Some(0);

        Self::from_genesis(genesis).expect("Default genesis is valid")
    }
}

// Implement EthChainSpec trait (required by reth)
impl EthChainSpec for LoadChainSpec {
    type Header = alloy_consensus::Header;

    fn chain(&self) -> Chain {
        self.inner.chain()
    }

    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> alloy_eips::eip1559::BaseFeeParams {
        self.inner.base_fee_params_at_timestamp(timestamp)
    }

    fn blob_params_at_timestamp(&self, timestamp: u64) -> Option<BlobParams> {
        self.inner.blob_params_at_timestamp(timestamp)
    }

    fn deposit_contract(&self) -> Option<&reth_chainspec::DepositContract> {
        self.inner.deposit_contract()
    }

    fn genesis_hash(&self) -> alloy_primitives::B256 {
        self.inner.genesis_hash()
    }

    fn prune_delete_limit(&self) -> usize {
        self.inner.prune_delete_limit()
    }

    fn display_hardforks(&self) -> Box<dyn std::fmt::Display> {
        Box::new(self.inner.display_hardforks())
    }

    fn genesis_header(&self) -> &Self::Header {
        self.inner.genesis_header()
    }

    fn genesis(&self) -> &Genesis {
        self.inner.genesis()
    }

    fn bootnodes(&self) -> Option<Vec<reth_network_peers::NodeRecord>> {
        self.inner.bootnodes()
    }

    fn final_paris_total_difficulty(&self) -> Option<U256> {
        self.inner.final_paris_total_difficulty()
    }
}

impl reth_evm::eth::spec::EthExecutorSpec for LoadChainSpec {
    fn deposit_contract_address(&self) -> Option<alloy_primitives::Address> {
        self.deposit_contract().map(|contract| contract.address)
    }
}

// Implement EthereumHardforks trait
impl EthereumHardforks for LoadChainSpec {
    fn ethereum_fork_activation(&self, fork: reth::chainspec::EthereumHardfork) -> ForkCondition {
        self.inner.ethereum_fork_activation(fork)
    }
}

// Implement Hardforks trait
impl Hardforks for LoadChainSpec {
    fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition {
        self.inner.fork(fork)
    }

    fn forks_iter(&self) -> impl Iterator<Item = (&dyn Hardfork, ForkCondition)> {
        self.inner.forks_iter()
    }

    fn fork_id(&self, head: &Head) -> ForkId {
        self.inner.fork_id(head)
    }

    fn latest_fork_id(&self) -> ForkId {
        self.inner.latest_fork_id()
    }

    fn fork_filter(&self, head: Head) -> ForkFilter {
        self.inner.fork_filter(head)
    }
}

/// Chain spec parser for Load Network (CLI integration)
#[derive(Debug, Clone, Default)]
pub struct LoadChainSpecParser;

const LOAD_DEV_GENESIS_JSON: &str = include_str!("../../etc/load-dev-genesis.json");
const LOAD_MAINNET_GENESIS_JSON: &str = include_str!("../../etc/load-mainnet-genesis.json");

impl ChainSpecParser for LoadChainSpecParser {
    type ChainSpec = LoadChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = &["load", "load-dev"];

    fn parse(s: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
        match s {
            "load-dev" => {
                info!("Using built-in Load dev chain spec");
                let genesis: Genesis = serde_json::from_str(LOAD_DEV_GENESIS_JSON)?;
                Ok(Arc::new(LoadChainSpec::from_genesis(genesis)?))
            }
            "load" => {
                info!("Using built-in Load mainnet chain spec");
                let genesis: Genesis = serde_json::from_str(LOAD_MAINNET_GENESIS_JSON)?;
                Ok(Arc::new(LoadChainSpec::from_genesis(genesis)?))
            }
            path => {
                info!(path, "Parsing Load chain spec from genesis file");
                let genesis = reth_cli::chainspec::parse_genesis(path)
                    .with_context(|| format!("Failed to parse genesis from {}", path))?;
                Ok(Arc::new(LoadChainSpec::from_genesis(genesis)?))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_chain_spec() {
        let spec = LoadChainSpec::default();
        assert_eq!(spec.chain().id(), 16_383);
        assert!(spec.is_cancun_active_at_timestamp(0));
        assert!(spec.is_prague_active_at_timestamp(0));
    }

    #[test]
    fn test_blob_params_at_genesis() {
        let spec = LoadChainSpec::default();
        let params = spec.blob_params_at_timestamp(0).expect("Cancun active at genesis");
        assert_eq!(params.max_blob_count, LOAD_MAX_BLOB_COUNT);
        assert_eq!(params.target_blob_count, LOAD_TARGET_BLOB_COUNT);
        assert_eq!(params.max_blobs_per_tx, LOAD_MAX_BLOBS_PER_TX);
        assert_eq!(params.update_fraction, LOAD_BLOB_UPDATE_FRACTION);
    }

    #[test]
    fn test_genesis_validation_requires_cancun_at_zero() {
        let mut genesis = Genesis::default();
        genesis.config.cancun_time = Some(100); // Wrong: should be 0
        genesis.config.terminal_total_difficulty = Some(U256::ZERO);

        let result = LoadChainSpec::from_genesis(genesis);
        assert!(result.is_err());
    }

    #[test]
    fn test_genesis_validation_accepts_no_ttd() {
        let mut genesis = Genesis::default();
        genesis.config.cancun_time = Some(0);
        genesis.config.terminal_total_difficulty = None; // PoS mode

        let result = LoadChainSpec::from_genesis(genesis);
        assert!(result.is_ok());
    }

    #[test]
    fn test_genesis_validation_accepts_zero_ttd() {
        let mut genesis = Genesis::default();
        genesis.config.cancun_time = Some(0);
        genesis.config.terminal_total_difficulty = Some(U256::ZERO);

        let result = LoadChainSpec::from_genesis(genesis);
        assert!(result.is_ok());
    }

    #[test]
    fn test_genesis_validation_rejects_nonzero_ttd() {
        let mut genesis = Genesis::default();
        genesis.config.cancun_time = Some(0);
        genesis.config.terminal_total_difficulty = Some(U256::from(1000));

        let result = LoadChainSpec::from_genesis(genesis);
        assert!(result.is_err());
    }

    #[test]
    fn test_hardforks_schedule() {
        let spec = LoadChainSpec::default();

        // Pre-Cancun forks at block 0
        assert!(spec.is_homestead_active_at_block(0));
        assert!(spec.is_berlin_active_at_block(0));
        assert!(spec.is_london_active_at_block(0));
        assert!(spec.is_paris_active_at_block(0));

        // Timestamp-based forks at timestamp 0
        assert!(spec.is_shanghai_active_at_timestamp(0));
        assert!(spec.is_cancun_active_at_timestamp(0));

        // Prague active at genesis
        assert!(spec.is_prague_active_at_timestamp(0));
    }

    #[test]
    fn test_fork_id_calculation() {
        let spec = LoadChainSpec::default();
        let fork_id = spec.latest_fork_id();
        assert_ne!(fork_id.hash, reth_chainspec::ForkHash([0u8; 4])); // Non-zero fork hash
    }

    #[test]
    fn test_parser_dev_chain() {
        let result = LoadChainSpecParser::parse("load-dev");
        assert!(result.is_ok());
        let spec = result.unwrap();
        assert_eq!(spec.chain().id(), 16_383);
    }

    #[test]
    fn test_parser_mainnet_builtin() {
        let result = LoadChainSpecParser::parse("load");
        assert!(result.is_ok());
        let spec = result.unwrap();
        assert_eq!(spec.chain().id(), 16_888);
    }

    #[test]
    fn prague_must_be_at_genesis() {
        let mut genesis = Genesis::default();
        genesis.config.cancun_time = Some(0);
        genesis.config.prague_time = Some(1);
        genesis.config.terminal_total_difficulty = Some(U256::ZERO);
        genesis.config.terminal_total_difficulty_passed = true;
        genesis.config.shanghai_time = Some(0);

        let result = LoadChainSpec::from_genesis(genesis);
        assert!(result.is_err());
        assert!(result.is_err());
    }

    #[test]
    fn shanghai_must_be_at_genesis() {
        let mut genesis = Genesis::default();
        genesis.config.cancun_time = Some(0);
        genesis.config.prague_time = Some(0);
        genesis.config.shanghai_time = Some(5);
        genesis.config.terminal_total_difficulty = Some(U256::ZERO);
        genesis.config.terminal_total_difficulty_passed = true;

        let result = LoadChainSpec::from_genesis(genesis);
        assert!(result.is_err());
    }

    #[test]
    fn merge_netsplit_must_be_zero() {
        let mut genesis = Genesis::default();
        genesis.config.cancun_time = Some(0);
        genesis.config.prague_time = Some(0);
        genesis.config.shanghai_time = Some(0);
        genesis.config.terminal_total_difficulty = Some(U256::ZERO);
        genesis.config.terminal_total_difficulty_passed = true;
        genesis.config.merge_netsplit_block = Some(1);

        let result = LoadChainSpec::from_genesis(genesis);
        assert!(result.is_err());
    }

    #[test]
    fn builtin_genesis_files_obey_guardrails() {
        fn assert_guardrails(genesis_json: &str) {
            let genesis: Genesis =
                serde_json::from_str(genesis_json).expect("built-in genesis parses");
            assert_eq!(genesis.config.cancun_time, Some(0));
            assert_eq!(genesis.config.prague_time, Some(0));
            assert_eq!(genesis.config.shanghai_time, Some(0));
            assert_eq!(genesis.config.terminal_total_difficulty, Some(U256::ZERO));
            assert!(genesis.config.terminal_total_difficulty_passed);
            assert_eq!(genesis.config.merge_netsplit_block, Some(0));
            LoadChainSpec::from_genesis(genesis).expect("guardrail-compliant genesis");
        }

        assert_guardrails(LOAD_DEV_GENESIS_JSON);
        assert_guardrails(LOAD_MAINNET_GENESIS_JSON);
    }
}
