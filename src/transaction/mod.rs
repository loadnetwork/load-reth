//! Load transaction helpers.
//!
//! Load currently reuses the standard Ethereum envelopes (Prague/Pectra feature
//! set) even though the protocol already bumps blob limits to 1024. Keeping
//! these helpers in one module makes it clear what the CL↔EL contract is today
//! and gives us a home for future Load-specific envelopes (e.g. DA metadata
//! extensions) without ripping through the rest of the codebase. Ultramarine
//! (the consensus client) ships the same envelopes over the Engine API, so any
//! deviation here must stay in lockstep with that project.

use alloy_consensus::Transaction as _;

/// Re-export the current transaction aliases.
pub use crate::primitives::{
    LoadTransaction as Transaction, LoadTransactionSigned as SignedTransaction,
};

/// Returns the number of blob versioned hashes carried by a signed transaction.
///
/// Used by the pool/payload builder to enforce Load's `max_blobs_per_tx = 32`
/// cap (Pectra semantics) independently of upstream Ethereum defaults.
pub fn blob_count(tx: &SignedTransaction) -> usize {
    tx.as_eip4844()
        .and_then(|signed| signed.tx().blob_versioned_hashes())
        .map(|hashes| hashes.len())
        .unwrap_or_default()
}
