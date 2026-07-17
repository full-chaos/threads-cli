use std::time::Duration;

pub(super) const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

pub(super) fn is_near_limit(value: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|json| json.as_object().cloned())
        .is_some_and(|object| {
            object
                .values()
                .filter_map(|number| number.as_f64())
                .any(|number| number >= 90.0)
        })
}

pub(super) fn backoff(base_ms: u64) -> Duration {
    Duration::from_millis((base_ms + jitter(base_ms)).min(MAX_RETRY_DELAY.as_millis() as u64))
}

pub(super) fn retry_after_delay(value: Option<&str>) -> Option<Duration> {
    value
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .map(Duration::from_secs)
        .map(|delay| delay.min(MAX_RETRY_DELAY))
}

fn jitter(base_ms: u64) -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0x9E3779B97F4A7C15);
    let mut value = SEED.load(Ordering::Relaxed);
    if value == 0 {
        value = 0xDEADBEEFCAFEBABE;
    }
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    SEED.store(value, Ordering::Relaxed);
    value % base_ms.max(1)
}
