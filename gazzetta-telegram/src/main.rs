use std::time::Duration;

use envconfig::Envconfig;
use gazzetta_common::MatchResult;
use log::{error, info};
use rsmq_async::{Rsmq, RsmqConnection, RsmqMessage};
use teloxide::requests::Requester;

#[derive(Envconfig)]
struct Config {
    #[envconfig(from = "TELEGRAM_BOT_TOKEN")]
    telegram_bot_token: String,
    #[envconfig(from = "TELEGRAM_CHAT_ID")]
    telegram_chat_id: String,
    #[envconfig(from = "GAZZETTA_REDIS_URL")]
    redis_url: String,
    #[envconfig(from = "GAZZETTA_NOTFIY_TASK_QUEUE")]
    notify_task_queue: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let config = Config::init_from_env()?;
    info!("Setup redis client to redis URL: {}", config.redis_url);
    let client = redis::Client::open(config.redis_url.as_str())?;

    loop {
        info!("Connecting to redis instance: {}", config.redis_url);
        let con = client.get_multiplexed_async_connection().await?;
        let mut rsmq = Rsmq::new_with_connection(con, false, None);
        let _ = rsmq
            .create_queue(&config.notify_task_queue, None, None, None)
            .await; // ignore if queue already exists

        match consume_new_items(&config, &mut rsmq).await {
            Ok(_) => (),
            Err(e) => {
                error!("Error: {:?}", e);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn consume_new_items(config: &Config, rqsm: &mut Rsmq) -> anyhow::Result<()> {
    info!(
        "Consumer waiting for new items in queue: {}",
        config.notify_task_queue
    );
    loop {
        let msg: Option<RsmqMessage<String>> = rqsm
            .receive_message(&config.notify_task_queue, None)
            .await?;
        let bot = teloxide::Bot::new(config.telegram_bot_token.clone());

        if let Some(msg) = msg {
            info!("Received new item: {:?}", msg);
            let match_result: MatchResult = serde_json::from_str(&msg.message)?;

            match bot
                .send_message(
                    config.telegram_chat_id.clone(),
                    format!(
                        "New item of interest from <<Gazzetta Ufficiale>>
**{}**
Description: {}

Matched Words: {:?}
Link: {}",
                        match_result.title.unwrap_or_default(),
                        match_result.summary.unwrap_or_default(),
                        match_result.matched_words,
                        match_result.url.unwrap_or_default()
                    ),
                )
                .await
            {
                Ok(_) => info!("Message sent successfully"),
                Err(e) => {
                    error!("Error sending message: {:?}", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            info!(
                "Processing done for: {}, deleting message from queue",
                &msg.id
            );
            rqsm.delete_message(&config.notify_task_queue, &msg.id)
                .await?;
        } else {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}
