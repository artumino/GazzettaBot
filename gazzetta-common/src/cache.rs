use std::{future::Future, time::Duration};

pub mod redis;

pub trait KeyValueCache {
    fn set(
        &mut self,
        key: &str,
        value: &str,
        duration: Duration,
    ) -> impl Future<Output = anyhow::Result<()>>;
    fn get(&mut self, key: &str) -> impl Future<Output = anyhow::Result<Option<String>>>;
}
