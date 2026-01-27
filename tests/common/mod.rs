use alloy_consensus::{SidecarBuilder, SimpleCoder};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_network::{
    eip2718::Encodable2718, Ethereum, EthereumWallet, TransactionBuilder, TransactionBuilder4844,
};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_rpc_types_engine::PayloadAttributes;
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use load_reth::{
    engine::payload::{LoadPayloadAttributes, LoadPayloadBuilderAttributes},
    LOAD_PREVRANDAO,
};
use reth_e2e_test_utils::wallet::Wallet;
use reth_payload_primitives::PayloadBuilderAttributes;

const DEV_GENESIS: &str = include_str!("../../etc/load-dev-genesis.json");
/// Deterministic blob fill byte for test fixtures.
#[allow(dead_code)]
const BLOB_FILL_BYTE: u8 = 0x42;

/// Returns a deterministic signer derived from the default mnemonic.
pub(crate) fn test_wallet() -> PrivateKeySigner {
    Wallet::new(1).wallet_gen().into_iter().next().expect("wallet fixture to produce signer")
}

/// Builds a Load payload attribute generator that enforces the PREVRANDAO constant.
pub(crate) fn load_payload_attributes(timestamp: u64) -> LoadPayloadBuilderAttributes {
    let rpc_attrs = PayloadAttributes {
        timestamp,
        prev_randao: B256::from(LOAD_PREVRANDAO),
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };

    LoadPayloadBuilderAttributes::try_new(B256::ZERO, LoadPayloadAttributes { inner: rpc_attrs }, 3)
        .expect("valid payload attributes")
}

/// Returns a dev genesis with the provided accounts funded generously.
pub(crate) fn funded_genesis(recipients: &[Address]) -> Genesis {
    let mut genesis: Genesis = serde_json::from_str(DEV_GENESIS).expect("valid dev genesis");
    for address in recipients {
        genesis.alloc.insert(
            *address,
            GenesisAccount {
                nonce: Some(0),
                balance: U256::from(10_000_000_000_000_000_000_u128),
                code: None,
                storage: None,
                private_key: None,
            },
        );
    }
    genesis
}

/// Builds a signed blob transaction with the requested blob count and nonce.
#[allow(dead_code)]
pub(crate) async fn blob_tx_with_nonce(
    chain_id: u64,
    wallet: PrivateKeySigner,
    nonce: u64,
    blob_count: usize,
) -> eyre::Result<Bytes> {
    use alloy_eips::eip4844::BYTES_PER_BLOB;

    let mut tx = TransactionRequest {
        nonce: Some(nonce),
        chain_id: Some(chain_id),
        gas: Some(300_000),
        max_fee_per_gas: Some(20e9 as u128),
        max_priority_fee_per_gas: Some(1e9 as u128),
        to: Some(TxKind::Call(Address::random())),
        value: Some(U256::from(100)),
        ..Default::default()
    };

    let mut builder = SidecarBuilder::<SimpleCoder>::new();
    for idx in 0..blob_count {
        let mut blob = vec![BLOB_FILL_BYTE; BYTES_PER_BLOB];
        blob[..8].copy_from_slice(&(idx as u64).to_le_bytes());
        builder.ingest(&blob);
    }
    tx.set_blob_sidecar(builder.build()?);
    tx.set_max_fee_per_blob_gas(15e9 as u128);

    let signer = EthereumWallet::from(wallet);
    let signed = <TransactionRequest as TransactionBuilder<Ethereum>>::build(tx, &signer).await?;
    Ok(signed.encoded_2718().into())
}
