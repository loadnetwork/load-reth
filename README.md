# load-reth

Execution client for the Load Network, built on the reth SDK. Load-reth implements
an Ethereum-compatible execution layer with enhanced data availability support
(up to 1024 blobs per block) and communicates with Ultramarine (consensus layer)
via Engine API v3.

## Status: Phase P2 / M5 Complete ✅

- ✅ **Chain Spec**: `LoadChainSpec` with custom blob params (max 1024, target 512)
- ✅ **Pool Builder**: Transaction pool with Load-specific blob cache (32,768 blobs ≈ 4.3 GB)
- ✅ **Node Composition**: SDK-style node using reth v1.9.3 components
- ✅ **CLI Binary**: `load-reth` with chain spec parser and reth integration
- ✅ **Genesis Config**: Default dev genesis (`etc/load-dev-genesis.json`)
- ✅ **Payload Builder & Engine Guards**: Load payload builder enforcing PREVRANDAO + blob caps, Engine API validators wired with Load types
- ✅ **Integration Harness (M5)**: FCU→getPayload round-trips (including optional 1024-blob stress), Prague gating (pre/post activation), blob ingress caps, and `engine_getBlobsV1` retrieval/request-weight coverage
- ✅ **Engine RPC Guardrails**: `engine_getBlobsV2` is explicitly gated to `UnsupportedFork` before Osaka and remains inert afterward until we store EIP-7594 sidecars; blob retrieval tests also cover multi-blob V1 responses plus empty/missing hash cases and the request-size limit
- 🛠️ **Next**: Observability & branding (Load-specific metrics/client string), eventual EIP-7594 support when required

## Key Features

| Feature | Ethereum | Load Network |
|---------|----------|--------------|
| Max blobs/block | 6 | **1024** |
| Target blobs/block | 3 | **512** |
| Blob cache size | ~384 blobs | **~32,768 blobs** |
| PREVRANDAO | Random | **Fixed (0x01)** |
| Consensus | Beacon chain | **Ultramarine (Tendermint)** |

## Architecture

```
Ultramarine (Consensus) ←─────────→ load-reth (Execution)
   Tendermint BFT              Engine API v3
   Blob metadata               Payload building
   ValueSync                   Transaction pool
                               EVM execution
```

### Engine API Contract (CL ↔ EL)

- `PayloadAttributes.prev_randao` **must be** `0x01` for all Engine API calls
- Cancun active at genesis (timestamp 0) for blob support
- Prague scheduled via genesis config for proposer metadata
- Terminal total difficulty = 0 (PoS mode from genesis)
- `engine_getBlobsV1` is supported today; `engine_getBlobsV2` is intentionally
  gated (`UnsupportedFork`) until Load stores Osaka sidecars.
- `web3_clientVersion` and Engine `engine_exchangeCapabilities` report the
  Load-specific identifier (`load-reth/v{version}-{sha}`) so CL tooling can
  distinguish EL builds.

## Installation

### Prerequisites

- Rust 1.82+ (see `rust-toolchain.toml`)
- Docker 24+ with BuildKit/buildx enabled (`DOCKER_BUILDKIT=1` is exported by the Makefile)
- [`cross`](https://github.com/cross-rs/cross) for multi-arch builds: `cargo install cross --locked`
- Optional developer tooling: `cargo-nextest`, `cargo-llvm-cov`, `typos-cli`

### Build from Source

```bash
cd load-reth
cargo build --release
```

The binary will be available at `target/release/load-reth`.

## Usage

### Initialize Node

```bash
# Initialize with Load dev genesis
load-reth init --chain etc/load-dev-genesis.json
```

### Run Node

```bash
# Basic node with HTTP RPC
load-reth node \
  --chain etc/load-dev-genesis.json \
  --http \
  --http.api eth,net,web3,engine \
  --authrpc.jwtsecret /path/to/jwt.hex \
  --authrpc.port 8551
```

### Custom Blob Cache

```bash
# Override blob cache size (default: 32,768 blobs)
load-reth node \
  --chain etc/load-dev-genesis.json \
  --txpool.blob-cache-size 40000
```

### Available Commands

```bash
load-reth --help           # Show all commands
load-reth init --help      # Initialize datadir
load-reth node --help      # Run node
load-reth db stats         # Database statistics
```

## Docker & Compose

Build a container image with the included multi-stage Dockerfile:

```bash
cd load-reth
make docker-build-local               # builds docker.io/loadnetwork/load-reth:local (BuildKit required)
make docker-build-push-latest         # multi-arch build & push (requires cross + BuildKit + Docker buildx)
```

> **Note:** If your Docker daemon still defaults to the legacy builder, run `DOCKER_BUILDKIT=1 make docker-build-local`.

Sample `docker-compose` service (mounts genesis/JWT and exposes authrpc/http/metrics):

```yaml
services:
  load-reth0:
    image: docker.io/loadnetwork/load-reth:latest
    volumes:
      - ./rethdata/0:/data
      - ./assets:/assets
    command:
      [
        "node",
        "--datadir=/data",
        "--chain=/assets/genesis.json",
        "--authrpc.addr=0.0.0.0",
        "--authrpc.port=8551",
        "--authrpc.jwtsecret=/assets/jwt.hex",
        "--http",
        "--http.addr=0.0.0.0",
        "--http.port=8545",
        "--metrics=0.0.0.0:9001"
      ]
    ports:
      - "8545:8545"
      - "8551:8551"
      - "9001:9001"
```

Ultramarine’s devnet stack pulls the published load-reth image by default; provide the EL genesis path and JWT secret via the existing `assets` mounts.

## Metrics

Load-reth exports a handful of Load-prefixed Prometheus metrics via the
standard reth `/metrics` endpoint:

- `load_reth_engine_forkchoice_duration_seconds`,
  `load_reth_engine_get_payload_duration_seconds`,
  `load_reth_engine_new_payload_duration_seconds`
- `load_reth_engine_get_blobs_requests_total`,
  `load_reth_engine_get_blobs_hits_total`,
  `load_reth_engine_get_blobs_misses_total`
- `load_reth_blob_cache_items`,
  `load_reth_blob_cache_bytes`

These complement the default reth metrics so Ultramarine can correlate CL/EL
events (e.g. blob cache depth vs. consensus height).

## Security

See [`SECURITY.md`](SECURITY.md) for the current review checklist
(PREVRANDAO, blob caps, fork gates) and tooling expectations
(`cargo audit --deny warnings`, `cargo deny`). `make pr` runs the full set
(fmt, clippy, tests, docs, deny, audit) so contributors can match CI locally.
Any change that touches the Engine API, blob handling, or chain spec must be
reviewed against that list.  
`cargo audit` currently ignores three upstream advisories:

- `RUSTSEC-2025-0055` (`tracing-subscriber` ANSI-escape poisoning) via
  `ark-relations` → `revm-precompile`
- `RUSTSEC-2024-0388` (`derivative` crate unmaintained) via older `ark-ff`
- `RUSTSEC-2024-0436` (`paste` crate unmaintained) via `tikv-jemalloc-ctl` and
  `syn-solidity`

Both originate inside arkworks dependencies bundled with `revm-precompile`; once
those crates ship patched releases we can drop the overrides.

## Configuration

### Chain Spec

Load-reth uses genesis JSON files for chain configuration. The default dev
genesis is provided at `etc/load-dev-genesis.json`:

- **Chain ID**: 16383 (dev)
- **Shanghai/Cancun/Prague time**: 0 (active at genesis)
- **Blob pricing**: Pectra (EIP-7691) update fraction from genesis
- **Terminal total difficulty**: 0 (PoS from genesis)

### Custom Genesis

Create a custom genesis JSON following the format:

```json
{
  "config": {
    "chainId": 16383,
    "shanghaiTime": 0,
    "cancunTime": 0,
    "pragueTime": 0,
    "terminalTotalDifficulty": 0,
    "terminalTotalDifficultyPassed": true
  },
  "gasLimit": "0x1c9c380",
  "baseFeePerGas": "0x7",
  ...
}
```

## Testing

```bash
# Run all tests
cargo test

# Run specific module tests
cargo test --package load-reth chainspec
cargo test --package load-reth pool

# Run integration tests
cargo test --test integration
```

## Development

### Helper Targets

The Makefile mirrors the Ultramarine developer UX and now exposes a few extra helpers:

- `make build-reproducible` builds a deterministic Linux `load-reth` binary (`SOURCE_DATE_EPOCH`, remapped paths, static CRT flags).
- `make test-nextest` / `make cov` / `make cov-report-html` wrap `cargo-nextest` and `cargo-llvm-cov` for faster feedback + lcov/html artifacts.
- `make lint-typos` runs [`typos`](https://github.com/crate-ci/typos) to keep docs/config spell-checked (included in `make lint`/`make pr`).

`make ci` and `make pr` now run `fmt-check`, `clippy`, `sort-check`, `lint-typos`, `tests`, `docs`, `cargo deny`, and `cargo audit` (with the advisory override mentioned above) so local runs match CI.

### Module Structure

```
src/
├── chainspec/     # LoadChainSpec with custom blob params
├── pool/          # LoadPoolBuilder with blob cache sizing
├── engine/        # Load payload builder wiring/guards + payload types
├── node/          # Node composition (NodeTypes, ComponentsBuilder)
└── lib.rs         # Public API and re-exports
```

### Key Types

- `LoadChainSpec`: Chain specification with Load blob parameters
- `LoadNode`: Node type configuration (primitives, storage, engine)
- `LoadPoolBuilder`: Transaction pool builder with blob cache
- `LoadPayloadServiceBuilder`: Payload builder service using Load types/guards
- `LoadChainSpecParser`: CLI chain spec parser

## References

- **Ultramarine (CL)**: `../ultramarine/`
- **Upstream**: `../reth/` (Paradigm reth)

## Roadmap

### ✅ Phase P1: Chain Spec & SDK Integration
- Chain specification with custom blob params
- Transaction pool with Load-specific blob cache
- Node composition using reth SDK
- CLI binary with genesis parser

### 🔄 Phase P2: Payload Builder
- Custom payload builder with PREVRANDAO validation
- Blob bundle validation (max 1024 enforcement)
- `engine_getBlobsV1` implementation

### ⏳ Phase P3: Engine API & Testing
- Engine API integration tests
- Harness with mocked Ultramarine
- 1024-blob payload roundtrip tests

### ⏳ Phase P4: Production Readiness
- RPC add-ons for blob observability
- Metrics instrumentation
- Security hardening

## License

Apache-2.0 OR MIT
