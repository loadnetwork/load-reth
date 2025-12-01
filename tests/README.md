Integration test layout
=======================

The harness mirrors upstream `reth`’s structure: shared fixtures live in
`common/` and each scenario sits in its own file so we can add coverage without
duplicating boilerplate.

## Running Tests

- All scenarios: `cargo test --tests`
- Specific file: `cargo test --test <name>`
- Include ignored stress tests: `cargo test --tests -- --ignored`
- Typical runtime: ~12 s (the ignored 32×32 blob stress adds ~2 min and multiple GB of RAM)

## Current scenarios

1. `blob_happy.rs` – FCU → getPayload happy paths (rstest cases for 8 and 24 blobs) plus optional stress cases (guarded by `LOAD_BLOB_STRESS=1`) for near-cap 1024 blobs and over-cap submission (1056) to ensure the builder enforces the 1024 cap.
2. `blob_caps.rs` – Negative coverage for blob limits: rejecting >32 blobs per tx at pool ingress and rejecting payloads with >1024 versioned hashes.
3. `payload_attrs.rs` – Prague gating regression: payloads carrying Prague execution requests are rejected before activation and (with empty requests to keep the block hash stable) accepted after activation; TODO left in-code for non-empty requests once the builder exposes request-aware sealing.
4. `engine_guards.rs` – Forkchoice ingress rejects payload attributes where `prev_randao != LOAD_PREVRANDAO` and accepts the constant value.
5. `blob_retrieval.rs` – Exercises `engine_getBlobsV1` (multi-blob responses, empty/missing hash handling, request-size guard) and `engine_getBlobsV2` rejection before Osaka (`UnsupportedFork`) using an authenticated Engine RPC client (runs in a dedicated high-stack thread because blobs are 131 KB each).

Upcoming work:

* Layer Load-specific RPC/metrics assertions once observability work lands (payload build latency, blob cache size, blob retrieval counts).
* Upgrade the Prague-after-activation acceptance test to drive non-empty requests once the payload builder exposes request-aware sealing (currently TODO’d in-code).
* Flip the `engine_getBlobsV2` expectation (and add positive coverage) once Load begins storing EIP-7594 sidecars post-Osaka.
