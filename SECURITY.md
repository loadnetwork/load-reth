# Security & Review Checklist

Load-reth inherits most security posture from upstream `reth`, but the Load
fork introduces a few non-negotiable invariants. Every change touching the
execution engine, network, or RPC layers must be reviewed against this list.

## Core Invariants

1. **PREVRANDAO is fixed to 0x01**  
   - Enforced at payload validation (`src/engine/validator.rs`) and RPC
     ingress (`src/engine/rpc.rs`).  
   - Any code path sending `PayloadAttributes` or `ExecutionPayload`s must keep
     the constant intact.

2. **Blob limits remain 32/tx and 1024/block**  
   - Pool ingress guards (`src/pool/mod.rs`) and payload builder logic cap
     blobs per Tx/Block. Never relax without protocol sign-off.

3. **Fork gates**  
   - Prague-only features (execution requests) stay behind the fork check.  
   - Osaka / `engine_getBlobsV2` stays `UnsupportedFork` until we store EIP-7594
     sidecars.

4. **Chain spec guardrails**  
   - Cancun/Prague at timestamp 0, merge-at-genesis (TTD=0,
     `merge_netsplit_block = 0`).  
   - Any new built-in genesis must pass `LoadChainSpec::from_genesis`.

5. **Engine ↔ Ultramarine contract**  
   - `forkchoiceUpdatedV3 → getPayloadV3 → newPayloadV3` flow must remain
     deterministic.  
   - `engine_getBlobsV1` serves only short-lived sidecars; long-term retention
     stays on Ultramarine.

## Tooling Expectations

- `cargo fmt`, `cargo clippy --all-targets --all-features`, and
  `cargo test --tests` must pass locally before opening a PR.
- Run `cargo deny check` and `cargo audit --deny warnings`; CI enforces both.
- Keep dependencies pinned to `reth v1.9.3` unless the bump is reviewed by the
  EL lead (reflect the change in `PLAN.md` and `Cargo.lock`).

### Advisory Exceptions

- `RUSTSEC-2025-0055` (`tracing-subscriber` ANSI escape poisoning) is currently
  ignored because `ark-relations 0.5.x` (pulled in via `revm-precompile`) has not
  shipped a build that depends on `tracing-subscriber >= 0.3.20`. Once arkworks
  publishes a patched release, remove the override.
- `RUSTSEC-2024-0388` (`derivative` crate unmaintained) is ignored for the same
  reason: `ark-ff` still depends on it and no maintained replacement exists yet.
- `RUSTSEC-2024-0436` (`paste` crate unmaintained) transits `tikv-jemalloc-ctl`
  and `syn-solidity`; upstream is aware but no replacement exists yet.

## Reporting

Security bugs should be reported privately to the Load Network core team. Do
not open public issues for vulnerabilities until coordinated disclosure is
complete.
