// Ideally this would use a relational DB to figure out who and what we need to notify
// Right now this is hardcoded to what i need

use std::{cell::OnceCell, time::Duration};

use envconfig::Envconfig;
use gazzetta_common::{cache::KeyValueCache, Item, ItemHeader, MatchResult};
use log::{error, info, warn};
use rsmq_async::{Rsmq, RsmqConnection, RsmqMessage};

#[derive(Envconfig)]
struct Config {
    #[envconfig(from = "SEARCH_KEYWORDS")]
    search_keywords: String,
    #[envconfig(from = "GAZZETTA_REDIS_URL")]
    redis_url: String,
    #[envconfig(from = "GAZZETTA_NEW_ARTICLE_TASK_QUEUE")]
    new_article_task_queue: String,
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
        let mut con = client.get_multiplexed_async_connection().await?;
        let job_con = con.clone();
        let mut rsmq = Rsmq::new_with_connection(job_con, false, None);
        let _ = rsmq
            .create_queue(&config.new_article_task_queue, None, None, None)
            .await; // ignore if queue already exists
        let _ = rsmq
            .create_queue(&config.notify_task_queue, None, None, None)
            .await; // ignore if queue already exists

        match consume_new_items(&config, &mut rsmq, &mut con).await {
            Ok(_) => (),
            Err(e) => {
                error!("Error: {:?}", e);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn consume_new_items(
    config: &Config,
    rqsm: &mut Rsmq,
    cache: &mut impl KeyValueCache,
) -> anyhow::Result<()> {
    loop {
        info!(
            "Consumer waiting for new items in queue: {}",
            config.new_article_task_queue
        );
        let msg: Option<RsmqMessage<String>> = rqsm
            .receive_message(&config.new_article_task_queue, None)
            .await?;
        if let Some(msg) = msg {
            info!("Received new item: {:?}", msg);
            let item_header: ItemHeader = serde_json::from_str(&msg.message)?;

            info!("Processing : {}", &item_header.key);
            let item = cache.get(&item_header.key).await?;

            if item.is_none() {
                warn!("Item {} not found in cache, skipping...", &item_header.key);
                continue;
            }

            let item: Item = serde_json::from_str(item.unwrap().as_str())?;
            let search_keywords = OnceCell::new();
            let search_keywords = search_keywords
                .get_or_init(|| config.search_keywords.split(',').collect::<Vec<&str>>());

            let matched_words: Vec<_> = search_keywords
                .iter()
                .filter(|keyword| {
                    item.title
                        .as_ref()
                        .map(|title| title.contains(*keyword))
                        .unwrap_or(false)
                        || item
                            .summary
                            .as_ref()
                            .map(|desc| desc.contains(*keyword))
                            .unwrap_or(false)
                        || item
                            .content
                            .as_ref()
                            .map(|content| content.contains(*keyword))
                            .unwrap_or(false)
                })
                .copied()
                .collect();

            // TODO: Some outboxing here would be needed, actually
            if !matched_words.is_empty() {
                info!("Item matches search keywords: {:?}", matched_words);
                rqsm.send_message(
                    &config.notify_task_queue,
                    serde_json::to_string(&MatchResult {
                        matched_words: matched_words
                            .into_iter()
                            .map(|word| word.to_string())
                            .collect(),
                        title: item.title,
                        summary: item.summary,
                        url: item.url,
                    })?,
                    None,
                )
                .await?;
            }

            info!(
                "Processing done for: {}, deleting message from queue",
                &msg.id
            );
            rqsm.delete_message(&config.new_article_task_queue, &msg.id)
                .await?;
        } else {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}
