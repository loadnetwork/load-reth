//! Load-reth specific Prometheus metrics helpers.

use std::time::Duration;

use reth_metrics::metrics::{self, Counter, Gauge, Histogram};

/// Engine RPC latency + getBlobs counters.
#[derive(Debug, Clone)]
pub struct LoadEngineRpcMetrics {
    forkchoice_latency: Histogram,
    get_payload_latency: Histogram,
    new_payload_latency: Histogram,
    get_blobs_requests: Counter,
    get_blobs_hits: Counter,
    get_blobs_misses: Counter,
}

impl LoadEngineRpcMetrics {
    pub fn new() -> Self {
        Self {
            forkchoice_latency: metrics::histogram!(
                "load_reth_engine_forkchoice_duration_seconds",
                "stage" => "forkchoiceUpdatedV3"
            ),
            get_payload_latency: metrics::histogram!(
                "load_reth_engine_get_payload_duration_seconds",
                "stage" => "getPayload"
            ),
            new_payload_latency: metrics::histogram!(
                "load_reth_engine_new_payload_duration_seconds",
                "stage" => "newPayload"
            ),
            get_blobs_requests: metrics::counter!("load_reth_engine_get_blobs_requests_total"),
            get_blobs_hits: metrics::counter!("load_reth_engine_get_blobs_hits_total"),
            get_blobs_misses: metrics::counter!("load_reth_engine_get_blobs_misses_total"),
        }
    }

    pub fn record_forkchoice(&self, duration: Duration) {
        self.forkchoice_latency.record(duration.as_secs_f64());
    }

    pub fn record_get_payload(&self, duration: Duration) {
        self.get_payload_latency.record(duration.as_secs_f64());
    }

    pub fn record_new_payload(&self, duration: Duration) {
        self.new_payload_latency.record(duration.as_secs_f64());
    }

    pub fn record_get_blobs(&self, hits: u64, misses: u64) {
        self.get_blobs_requests.increment(1);
        self.get_blobs_hits.increment(hits);
        self.get_blobs_misses.increment(misses);
    }
}

impl Default for LoadEngineRpcMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Blob cache occupancy gauges.
#[derive(Debug, Clone)]
pub struct LoadBlobCacheMetrics {
    items: Gauge,
    bytes: Gauge,
}

impl LoadBlobCacheMetrics {
    pub fn new() -> Self {
        Self {
            items: metrics::gauge!("load_reth_blob_cache_items"),
            bytes: metrics::gauge!("load_reth_blob_cache_bytes"),
        }
    }

    pub fn record(&self, items: usize, bytes: Option<u64>) {
        self.items.set(items as f64);
        if let Some(size) = bytes {
            self.bytes.set(size as f64);
        }
    }
}

impl Default for LoadBlobCacheMetrics {
    fn default() -> Self {
        Self::new()
    }
}
