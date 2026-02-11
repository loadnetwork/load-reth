//! Method-specific RPC backpressure middleware for hot paths.
//!
//! This layer applies deterministic, fail-fast concurrency caps to selected
//! RPC methods to protect node responsiveness under sustained overload.

use std::{
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use jsonrpsee::{
    core::middleware::{Batch, BatchEntry, Notification, RpcServiceT},
    types::{error::ErrorObjectOwned, Request},
    BatchResponseBuilder, MethodResponse,
};
use reth_metrics::metrics::{self, Counter, Gauge};
use serde::Serialize;
use tower::Layer;
use tracing::{info, warn};

const RPC_BACKPRESSURE_OVERLOAD_CODE: i32 = -32005;
const RPC_BACKPRESSURE_OVERLOAD_MESSAGE: &str =
    "load-reth RPC overload: method concurrency limit reached";

const ENV_SEND_RAW_TX_LIMIT: &str = "LOAD_RETH_RPC_SEND_RAW_TX_LIMIT";
const ENV_GET_TRANSACTION_COUNT_LIMIT: &str = "LOAD_RETH_RPC_GET_TRANSACTION_COUNT_LIMIT";
const ENV_SEND_RAW_TX_SYNC_LIMIT: &str = "LOAD_RETH_RPC_SEND_RAW_TX_SYNC_LIMIT";
const ENV_BATCH_RESPONSE_LIMIT_MB: &str = "LOAD_RETH_RPC_BATCH_RESPONSE_LIMIT_MB";

const DEFAULT_SEND_RAW_TX_LIMIT: usize = 1024;
const DEFAULT_GET_TRANSACTION_COUNT_LIMIT: usize = 2048;
const DEFAULT_SEND_RAW_TX_SYNC_LIMIT: usize = 256;
const DEFAULT_BATCH_RESPONSE_LIMIT_MB: usize = 200;

/// Env-configurable concurrency limits for guarded RPC methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadRpcBackpressureConfig {
    /// Max inflight `eth_sendRawTransaction` requests.
    pub send_raw_transaction_limit: usize,
    /// Max inflight `eth_getTransactionCount` requests.
    pub get_transaction_count_limit: usize,
    /// Max inflight `eth_sendRawTransactionSync` requests.
    pub send_raw_transaction_sync_limit: usize,
    /// Max batch response size used by middleware when rebuilding guarded batches (bytes).
    pub batch_response_limit_bytes: usize,
}

impl Default for LoadRpcBackpressureConfig {
    fn default() -> Self {
        Self {
            send_raw_transaction_limit: DEFAULT_SEND_RAW_TX_LIMIT,
            get_transaction_count_limit: DEFAULT_GET_TRANSACTION_COUNT_LIMIT,
            send_raw_transaction_sync_limit: DEFAULT_SEND_RAW_TX_SYNC_LIMIT,
            batch_response_limit_bytes: DEFAULT_BATCH_RESPONSE_LIMIT_MB * 1024 * 1024,
        }
    }
}

impl LoadRpcBackpressureConfig {
    /// Loads config from environment variables, falling back to defaults.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        let config = Self {
            send_raw_transaction_limit: parse_env_limit(
                ENV_SEND_RAW_TX_LIMIT,
                defaults.send_raw_transaction_limit,
            ),
            get_transaction_count_limit: parse_env_limit(
                ENV_GET_TRANSACTION_COUNT_LIMIT,
                defaults.get_transaction_count_limit,
            ),
            send_raw_transaction_sync_limit: parse_env_limit(
                ENV_SEND_RAW_TX_SYNC_LIMIT,
                defaults.send_raw_transaction_sync_limit,
            ),
            batch_response_limit_bytes: parse_env_mb_as_bytes(
                ENV_BATCH_RESPONSE_LIMIT_MB,
                DEFAULT_BATCH_RESPONSE_LIMIT_MB,
            ),
        };

        info!(
            target: "load_reth::rpc",
            send_raw_transaction_limit = config.send_raw_transaction_limit,
            get_transaction_count_limit = config.get_transaction_count_limit,
            send_raw_transaction_sync_limit = config.send_raw_transaction_sync_limit,
            batch_response_limit_bytes = config.batch_response_limit_bytes,
            "Configured RPC backpressure limits"
        );

        config
    }

    const fn limit_for(self, method: GuardedMethod) -> usize {
        match method {
            GuardedMethod::SendRawTransaction => self.send_raw_transaction_limit,
            GuardedMethod::GetTransactionCount => self.get_transaction_count_limit,
            GuardedMethod::SendRawTransactionSync => self.send_raw_transaction_sync_limit,
        }
    }
}

/// Tower RPC layer that applies method-specific backpressure.
#[derive(Debug, Clone)]
pub struct LoadRpcBackpressureLayer {
    state: Arc<LoadRpcBackpressureState>,
}

impl LoadRpcBackpressureLayer {
    /// Creates a new layer from explicit config.
    pub fn new(config: LoadRpcBackpressureConfig) -> Self {
        Self { state: Arc::new(LoadRpcBackpressureState::new(config)) }
    }

    /// Creates a new layer using env vars + defaults.
    pub fn from_env() -> Self {
        Self::new(LoadRpcBackpressureConfig::from_env())
    }
}

impl<S> Layer<S> for LoadRpcBackpressureLayer {
    type Service = LoadRpcBackpressureService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LoadRpcBackpressureService { inner, state: self.state.clone() }
    }
}

/// Service wrapper for RPC backpressure.
#[derive(Debug, Clone)]
pub struct LoadRpcBackpressureService<S> {
    inner: S,
    state: Arc<LoadRpcBackpressureState>,
}

impl<S> RpcServiceT for LoadRpcBackpressureService<S>
where
    S: RpcServiceT<
            MethodResponse = MethodResponse,
            BatchResponse = MethodResponse,
            NotificationResponse = MethodResponse,
        > + Send
        + Sync
        + Clone
        + 'static,
{
    type MethodResponse = MethodResponse;
    type BatchResponse = MethodResponse;
    type NotificationResponse = MethodResponse;

    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
        let inner = self.inner.clone();
        let state = self.state.clone();
        async move {
            if let Some(method) = state.classify_method(req.method_name()) {
                let Some(_permit) = state.try_acquire(method) else {
                    return state.overload_response(req, method)
                };
                return inner.call(req).await
            }
            inner.call(req).await
        }
    }

    fn batch<'a>(&self, req: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
        let this = self.clone();
        async move {
            let has_guarded_call = req.iter().flatten().any(|entry| {
                matches!(
                    entry,
                    BatchEntry::Call(call)
                        if this.state.classify_method(call.method_name()).is_some()
                )
            });
            if !has_guarded_call {
                return this.inner.batch(req).await;
            }

            let mut batch_rp =
                BatchResponseBuilder::new_with_limit(this.state.config.batch_response_limit_bytes);
            let mut got_notification = false;

            for batch_entry in req {
                match batch_entry {
                    Ok(BatchEntry::Call(call)) => {
                        let rp = this.call(call).await;
                        if let Err(err) = batch_rp.append(rp) {
                            return err;
                        }
                    }
                    Ok(BatchEntry::Notification(notification)) => {
                        got_notification = true;
                        this.notification(notification).await;
                    }
                    Err(err) => {
                        let (err, id) = err.into_parts();
                        let rp = MethodResponse::error(id, err);
                        if let Err(err) = batch_rp.append(rp) {
                            return err;
                        }
                    }
                }
            }

            if batch_rp.is_empty() && got_notification {
                MethodResponse::notification()
            } else {
                MethodResponse::from_batch(batch_rp.finish())
            }
        }
    }

    fn notification<'a>(
        &self,
        n: Notification<'a>,
    ) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
        self.inner.notification(n)
    }
}

#[derive(Debug)]
struct LoadRpcBackpressureState {
    config: LoadRpcBackpressureConfig,
    metrics: LoadRpcBackpressureMetrics,
    send_raw_transaction: MethodGate,
    get_transaction_count: MethodGate,
    send_raw_transaction_sync: MethodGate,
}

impl LoadRpcBackpressureState {
    fn new(config: LoadRpcBackpressureConfig) -> Self {
        Self {
            config,
            metrics: LoadRpcBackpressureMetrics::new(),
            send_raw_transaction: MethodGate::new(config.send_raw_transaction_limit),
            get_transaction_count: MethodGate::new(config.get_transaction_count_limit),
            send_raw_transaction_sync: MethodGate::new(config.send_raw_transaction_sync_limit),
        }
    }

    fn classify_method(&self, method_name: &str) -> Option<GuardedMethod> {
        let method = GuardedMethod::from_rpc_method(method_name)?;
        (self.config.limit_for(method) > 0).then_some(method)
    }

    fn try_acquire(self: &Arc<Self>, method: GuardedMethod) -> Option<InflightPermit> {
        let gate = self.gate(method);
        loop {
            let inflight = gate.inflight.load(Ordering::Relaxed);
            if inflight >= gate.limit {
                self.metrics.record_rejected(method);
                return None;
            }
            if gate
                .inflight
                .compare_exchange_weak(inflight, inflight + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                self.metrics.record_accepted(method);
                self.metrics.record_inflight(method, inflight + 1);
                return Some(InflightPermit { state: self.clone(), method });
            }
        }
    }

    fn release(&self, method: GuardedMethod) {
        let gate = self.gate(method);
        let prev = gate.inflight.fetch_sub(1, Ordering::AcqRel);
        let next = prev.saturating_sub(1);
        if prev == 0 {
            gate.inflight.store(0, Ordering::Release);
        }
        self.metrics.record_inflight(method, next);
    }

    fn gate(&self, method: GuardedMethod) -> &MethodGate {
        match method {
            GuardedMethod::SendRawTransaction => &self.send_raw_transaction,
            GuardedMethod::GetTransactionCount => &self.get_transaction_count,
            GuardedMethod::SendRawTransactionSync => &self.send_raw_transaction_sync,
        }
    }

    fn overload_response<'a>(&self, req: Request<'a>, method: GuardedMethod) -> MethodResponse {
        let error = ErrorObjectOwned::owned(
            RPC_BACKPRESSURE_OVERLOAD_CODE,
            RPC_BACKPRESSURE_OVERLOAD_MESSAGE,
            Some(OverloadErrorData {
                method: method.rpc_method_name(),
                limit: self.config.limit_for(method),
            }),
        );
        MethodResponse::error(req.id, error)
    }
}

#[derive(Debug)]
struct MethodGate {
    limit: usize,
    inflight: AtomicUsize,
}

impl MethodGate {
    const fn new(limit: usize) -> Self {
        Self { limit, inflight: AtomicUsize::new(0) }
    }
}

#[derive(Debug)]
struct InflightPermit {
    state: Arc<LoadRpcBackpressureState>,
    method: GuardedMethod,
}

impl Drop for InflightPermit {
    fn drop(&mut self) {
        self.state.release(self.method);
    }
}

#[derive(Debug, Clone)]
struct LoadRpcBackpressureMetrics {
    accepted_send_raw_transaction: Counter,
    accepted_get_transaction_count: Counter,
    accepted_send_raw_transaction_sync: Counter,
    rejected_send_raw_transaction: Counter,
    rejected_get_transaction_count: Counter,
    rejected_send_raw_transaction_sync: Counter,
    inflight_send_raw_transaction: Gauge,
    inflight_get_transaction_count: Gauge,
    inflight_send_raw_transaction_sync: Gauge,
}

impl LoadRpcBackpressureMetrics {
    fn new() -> Self {
        Self {
            accepted_send_raw_transaction: metrics::counter!(
                "load_reth_rpc_backpressure_accepted_total",
                "method" => GuardedMethod::SendRawTransaction.rpc_method_name()
            ),
            accepted_get_transaction_count: metrics::counter!(
                "load_reth_rpc_backpressure_accepted_total",
                "method" => GuardedMethod::GetTransactionCount.rpc_method_name()
            ),
            accepted_send_raw_transaction_sync: metrics::counter!(
                "load_reth_rpc_backpressure_accepted_total",
                "method" => GuardedMethod::SendRawTransactionSync.rpc_method_name()
            ),
            rejected_send_raw_transaction: metrics::counter!(
                "load_reth_rpc_backpressure_rejected_total",
                "method" => GuardedMethod::SendRawTransaction.rpc_method_name()
            ),
            rejected_get_transaction_count: metrics::counter!(
                "load_reth_rpc_backpressure_rejected_total",
                "method" => GuardedMethod::GetTransactionCount.rpc_method_name()
            ),
            rejected_send_raw_transaction_sync: metrics::counter!(
                "load_reth_rpc_backpressure_rejected_total",
                "method" => GuardedMethod::SendRawTransactionSync.rpc_method_name()
            ),
            inflight_send_raw_transaction: metrics::gauge!(
                "load_reth_rpc_backpressure_inflight",
                "method" => GuardedMethod::SendRawTransaction.rpc_method_name()
            ),
            inflight_get_transaction_count: metrics::gauge!(
                "load_reth_rpc_backpressure_inflight",
                "method" => GuardedMethod::GetTransactionCount.rpc_method_name()
            ),
            inflight_send_raw_transaction_sync: metrics::gauge!(
                "load_reth_rpc_backpressure_inflight",
                "method" => GuardedMethod::SendRawTransactionSync.rpc_method_name()
            ),
        }
    }

    fn record_accepted(&self, method: GuardedMethod) {
        self.accepted_counter(method).increment(1);
    }

    fn record_rejected(&self, method: GuardedMethod) {
        self.rejected_counter(method).increment(1);
    }

    fn record_inflight(&self, method: GuardedMethod, inflight: usize) {
        self.inflight_gauge(method).set(inflight as f64);
    }

    fn accepted_counter(&self, method: GuardedMethod) -> &Counter {
        match method {
            GuardedMethod::SendRawTransaction => &self.accepted_send_raw_transaction,
            GuardedMethod::GetTransactionCount => &self.accepted_get_transaction_count,
            GuardedMethod::SendRawTransactionSync => &self.accepted_send_raw_transaction_sync,
        }
    }

    fn rejected_counter(&self, method: GuardedMethod) -> &Counter {
        match method {
            GuardedMethod::SendRawTransaction => &self.rejected_send_raw_transaction,
            GuardedMethod::GetTransactionCount => &self.rejected_get_transaction_count,
            GuardedMethod::SendRawTransactionSync => &self.rejected_send_raw_transaction_sync,
        }
    }

    fn inflight_gauge(&self, method: GuardedMethod) -> &Gauge {
        match method {
            GuardedMethod::SendRawTransaction => &self.inflight_send_raw_transaction,
            GuardedMethod::GetTransactionCount => &self.inflight_get_transaction_count,
            GuardedMethod::SendRawTransactionSync => &self.inflight_send_raw_transaction_sync,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuardedMethod {
    SendRawTransaction,
    GetTransactionCount,
    SendRawTransactionSync,
}

impl GuardedMethod {
    fn from_rpc_method(method: &str) -> Option<Self> {
        match method {
            "eth_sendRawTransaction" => Some(Self::SendRawTransaction),
            "eth_getTransactionCount" => Some(Self::GetTransactionCount),
            "eth_sendRawTransactionSync" => Some(Self::SendRawTransactionSync),
            _ => None,
        }
    }

    const fn rpc_method_name(self) -> &'static str {
        match self {
            Self::SendRawTransaction => "eth_sendRawTransaction",
            Self::GetTransactionCount => "eth_getTransactionCount",
            Self::SendRawTransactionSync => "eth_sendRawTransactionSync",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverloadErrorData {
    method: &'static str,
    limit: usize,
}

fn parse_env_limit(var: &str, default: usize) -> usize {
    match std::env::var(var) {
        Ok(value) => match value.parse::<usize>() {
            Ok(parsed) => parsed,
            Err(err) => {
                warn!(
                    target: "load_reth::rpc",
                    env_var = var,
                    raw_value = %value,
                    %err,
                    fallback = default,
                    "Invalid RPC backpressure limit env var, using default"
                );
                default
            }
        },
        Err(std::env::VarError::NotPresent) => default,
        Err(err) => {
            warn!(
                target: "load_reth::rpc",
                env_var = var,
                %err,
                fallback = default,
                "Failed reading RPC backpressure limit env var, using default"
            );
            default
        }
    }
}

fn parse_env_mb_as_bytes(var: &str, default_mb: usize) -> usize {
    parse_env_limit(var, default_mb).saturating_mul(1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::{
        GuardedMethod, LoadRpcBackpressureConfig, LoadRpcBackpressureLayer,
        LoadRpcBackpressureState,
    };

    #[test]
    fn classifies_required_methods() {
        let state = LoadRpcBackpressureLayer::new(LoadRpcBackpressureConfig::default()).state;

        assert_eq!(
            state.classify_method("eth_sendRawTransaction"),
            Some(GuardedMethod::SendRawTransaction)
        );
        assert_eq!(
            state.classify_method("eth_getTransactionCount"),
            Some(GuardedMethod::GetTransactionCount)
        );
        assert_eq!(
            state.classify_method("eth_sendRawTransactionSync"),
            Some(GuardedMethod::SendRawTransactionSync)
        );
        assert_eq!(state.classify_method("debug_traceTransaction"), None);
    }

    #[test]
    fn disabled_limit_skips_method_guard() {
        let cfg = LoadRpcBackpressureConfig {
            send_raw_transaction_limit: 1,
            get_transaction_count_limit: 0,
            send_raw_transaction_sync_limit: 1,
            batch_response_limit_bytes: 1024 * 1024,
        };
        let state = LoadRpcBackpressureState::new(cfg);

        assert_eq!(state.classify_method("eth_getTransactionCount"), None);
        assert_eq!(
            state.classify_method("eth_sendRawTransaction"),
            Some(GuardedMethod::SendRawTransaction)
        );
    }

    #[test]
    fn rejects_immediately_at_capacity() {
        let cfg = LoadRpcBackpressureConfig {
            send_raw_transaction_limit: 1,
            get_transaction_count_limit: 1,
            send_raw_transaction_sync_limit: 1,
            batch_response_limit_bytes: 1024 * 1024,
        };
        let state = std::sync::Arc::new(LoadRpcBackpressureState::new(cfg));

        let first = state
            .try_acquire(GuardedMethod::SendRawTransaction)
            .expect("first request should be admitted");
        let second = state.try_acquire(GuardedMethod::SendRawTransaction);
        assert!(second.is_none(), "second request must fail fast at limit");

        drop(first);
        let third = state.try_acquire(GuardedMethod::SendRawTransaction);
        assert!(third.is_some(), "slot should reopen immediately after release");
    }
}
