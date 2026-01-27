# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial load-reth implementation based on reth SDK v1.9.3
- **Hardcoded persistence-threshold=0**: Uses `DefaultEngineValues` API (reth v1.10.2+) to
  ensure immediate block persistence for 1-slot finality. Prevents race condition where
  finalized blocks may still be in memory on EL restart, leading to CL/EL desync (BUG-013).
- **Engine API V5 (Osaka)**: Added `engine_getBlobsV3` method with `BlobAndProofV2` support.
- Custom blob validation for Load Network (max 1024 blobs/block, target 512)
- Docker multi-arch support (amd64, arm64)
- CI/CD workflows with conventional commits enforcement
- Security scanning with cargo-deny, cargo-audit, and Trivy
- **Engine API V4 (Prague)**: Full support for `newPayloadV4`, `getPayloadV4`, and
  `forkchoiceUpdatedV3` with Prague execution requests. Load does not deploy EIP-6110/7002/7251
  system contracts, so execution always produces `EMPTY_REQUESTS_HASH`.
- **2 billion gas limit**: `LOAD_EXECUTION_GAS_LIMIT = 2_000_000_000` enforced across
  chain spec, genesis, and EVM configuration.
- **Load-specific metrics**: Prometheus metrics prefixed with `load_reth_` for Engine API
  latency (P95 histograms for newPayload/getPayload/forkchoice), blob cache occupancy,
  and `engine_getBlobsV1` hit/miss counters.
- **Grafana dashboard**: "Load Engine" row with 4 panels for EL observability.

### Changed
- **Upgraded to reth SDK v1.10.2** with the following API adaptations:
  - `PayloadAttributesBuilder::build` now takes `&SealedHeader` instead of `u64` timestamp
  - `ExecutionPayload` trait now requires `block_access_list()` and `transaction_count()` methods
  - `EngineApi::new` now requires a `network` argument for sync awareness
  - `EngineApiServer` now requires `get_blobs_v3` method implementation
  - `PayloadValidator` trait refactored: `convert_payload_to_block` is now the primary method,
    `ensure_well_formed_payload` has a default implementation that calls it
  - `NextBlockEnvAttributes` now includes `extra_data` field
  - `BestTransactionsAttributes::mark_invalid` now takes `&InvalidPoolTransactionError` (borrow)
  - `EthEvmConfig::with_extra_data` removed; extra_data flows via `NextBlockEnvAttributes`
- `LoadEvmConfig` now enforces Load blob parameters (`max_blobs_per_tx`, `update_fraction`,
  `target_blob_count`, `max_blob_count`) on every EVM environment construction.

### Deprecated
- N/A

### Removed
- N/A

### Fixed
- **BUG-013**: Fixed race condition with 1-slot finality by hardcoding `persistence_threshold=0`.
  See `ultramarine/docs/journal/BUG-013-chain-split-parent-mismatch.md` for details.
- Prague payload acceptance test now correctly uses empty requests (`Requests::default()`)
  to match Load's execution semantics (no system contract deposits/withdrawals/consolidations).

### Security
- N/A
