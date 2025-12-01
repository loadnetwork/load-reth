# Load Network Genesis Contract (EL ≤→ CL)

This file captures the canonical parameters shared between Ultramarine (CL) and
load-reth (EL). It is the single source of truth for built-in genesis files and
should stay in sync with Ultramarine’s docs.

## Networks & Parameters

| Network  | Chain ID | Cancun | Prague | Osaka | Blob Limits (max/target/tx) | Notes |
|----------|----------|--------|--------|-------|-----------------------------|-------|
| Dev      | 16383    | 0      | 0      | _N/A_ | 1024 / 512 / 32             | Shipped as `etc/load-dev-genesis.json` |
| Testnet* | 16888    | 0      | 0      | _N/A_ | 1024 / 512 / 32             | Shipped as `etc/load-mainnet-genesis.json` (placeholder values) |
| Mainnet† | _TBD_    | 0      | 0      | _N/A_ | 1024 / 512 / 32             | Replace chain ID/allocations before launch |

\* `load-mainnet-genesis.json` mirrors the future public chain. Update chain ID,
allocations, and timestamps once finalized.

\† Mainnet parameters inherit the same fork timings (Cancun/Prague at genesis)
and blob caps. PREVRANDAO is always fixed to `0x…01`.

## Global Requirements

- **PoS at genesis**: `terminalTotalDifficulty = 0`, `terminalTotalDifficultyPassed = true`,
  `merge_netsplit_block = 0`.
- **Fork scheduling**: every pre-Cancun fork at block 0; Shanghai/Cancun/Prague
  at timestamp 0.
- **Blob economics**: `max_blob_count = 1024`, `target_blob_count = 512`,
  `max_blobs_per_tx = 32`, update fraction = Pectra (5_007_716).
- **PREVRANDAO**: fixed constant `0x000…001` across every network.

Any change to these invariants must be reflected in this file, the JSON
genesis files under `etc/`, and Ultramarine’s mirrored documentation.
