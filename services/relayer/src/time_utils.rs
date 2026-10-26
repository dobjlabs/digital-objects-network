use std::time::{SystemTime, UNIX_EPOCH};

/// Current unix timestamp in seconds.
pub fn now_ts() -> i64 {
    #[cfg(test)]
    if let Some(now) = MODEL_TIME.with(|clock| clock.get()) {
        return now;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
thread_local! { static MODEL_TIME: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) }; }

#[cfg(test)]
pub(crate) fn with_model_time<T>(now: i64, operation: impl FnOnce() -> T) -> T {
    struct Restore(Option<i64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            MODEL_TIME.with(|clock| clock.set(self.0));
        }
    }
    let _restore = Restore(MODEL_TIME.with(|clock| clock.replace(Some(now))));
    operation()
}
