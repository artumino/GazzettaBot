use std::time::Duration;

use anyhow::Ok;
use redis::{AsyncCommands, SetOptions};

use super::KeyValueCache;

impl<T: AsyncCommands> KeyValueCache for T {
    async fn set(&mut self, key: &str, value: &str, duration: Duration) -> anyhow::Result<()> {
        self.set_options(
            key,
            value,
            SetOptions::default()
                .with_expiration(redis::SetExpiry::EX(duration.as_secs() as usize)),
        )
        .await?;
        Ok(())
    }

    async fn get(&mut self, key: &str) -> anyhow::Result<Option<String>> {
        let result: Option<String> = self.get(key).await?;
        Ok(result)
    }
    
    async fn set_expiration(&mut self, key: &str, duration: Duration) -> anyhow::Result<()> {
        self.expire(key, duration.as_secs() as i64).await?;
        Ok(())
    }
}
