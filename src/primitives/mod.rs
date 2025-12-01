//! Load primitive type definitions.
//!
//! At genesis Load speaks Prague/Pectra immediately but the underlying data
//! structures still match upstream Ethereum. Centralising the aliases here lets
//! us fork headers/transactions later (e.g. to embed DA proofs or proposer
//! metadata) without having to chase usages throughout the node.

use alloy_consensus::Header;
use reth::primitives::{
    Block as EthBlock, BlockBody as EthBlockBody, EthPrimitives, Receipt as EthReceipt,
    Transaction as EthTransaction, TransactionSigned as EthTransactionSigned,
};

/// Node primitive bundle (currently upstream Ethereum).
pub type LoadPrimitives = EthPrimitives;
/// Unsigned Load transaction.
pub type LoadTransaction = EthTransaction;
/// Signed Load transaction envelope.
pub type LoadTransactionSigned = EthTransactionSigned;
/// Load block type.
pub type LoadBlock = EthBlock;
/// Load block body type.
pub type LoadBlockBody = EthBlockBody;
/// Load receipt type.
pub type LoadReceipt = EthReceipt;
/// Load block header type.
pub type LoadHeader = Header;
