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
3. `payload_attrs.rs` – Prague gating regression: payloads carrying Prague execution requests are rejected before activation and accepted after activation. Load Network does not deploy the Prague system contracts (EIP-6110/7002/7251), so execution always produces `EMPTY_REQUESTS_HASH` and the test uses `Requests::default()`.
4. `engine_guards.rs` – Forkchoice ingress rejects payload attributes where `prev_randao != LOAD_PREVRANDAO` and accepts the constant value.
5. `blob_retrieval.rs` – Exercises `engine_getBlobsV1` (multi-blob responses, empty/missing hash handling, request-size guard) and `engine_getBlobsV2` rejection before Osaka (`UnsupportedFork`) using an authenticated Engine RPC client (runs in a dedicated high-stack thread because blobs are 131 KB each).
6. `persistence_restart.rs` – Ensures `persistence_threshold=0` is in effect by asserting canonical blocks are persisted immediately and survive a restart (guards the tip-2 loss scenario).

Upcoming work:

* Layer Load-specific RPC/metrics assertions into tests (payload build latency, blob cache size, blob retrieval counts are now exposed via `load_reth_*` Prometheus metrics).
* Flip the `engine_getBlobsV2` expectation (and add positive coverage) once Load begins storing EIP-7594 sidecars post-Osaka.
