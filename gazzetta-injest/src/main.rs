use std::{future::Future, time::Duration};

use anyhow::Context;
use backoff::{exponential::ExponentialBackoff, future::retry, SystemClock};
use envconfig::Envconfig;
use gazzetta_common::{cache::KeyValueCache, ItemHeader};
use log::info;
use reqwest::Url;
use rsmq_async::{Rsmq, RsmqConnection};
use rss::Item;

#[derive(Envconfig)]
struct Config {
    #[envconfig(from = "GAZZETTA_FEED_URL")]
    feed_url: String,
    #[envconfig(from = "GAZZETTA_POLL_INTERVAL_MS", default = "1800000")] // 30 minutes
    poll_interval_ms: u64,
    #[envconfig(from = "GAZZETTA_CACHE_EXPIRE_TIME_S", default = "172800")] // 2 days
    cache_expire_time_s: u64,
    #[envconfig(from = "GAZZETTA_REDIS_URL")]
    redis_url: String,
    #[envconfig(from = "GAZZETTA_NEW_ARTICLE_TASK_QUEUE")]
    redis_new_article_task_queue: String,
    #[envconfig(from = "SCRAPER_MAX_IFRAMES", default = "8")]
    max_iframes: u8,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let config = Config::init_from_env()?;
    info!(
        "Polling feed: {} every {} ms",
        config.feed_url, config.poll_interval_ms
    );

    info!("Setup redis client to redis URL: {}", config.redis_url);
    let client = redis::Client::open(config.redis_url.as_str())?;

    loop {
        retry(ExponentialBackoff::<SystemClock>::default(), || async {
            match setup_and_poll(&config, &client).await {
                Ok(_) => Ok(()),
                Err(e) => {
                    info!("Error: {:?}", e);
                    Err(backoff::Error::transient(e))
                }
            }
        })
        .await?;

        tokio::time::sleep(Duration::from_millis(config.poll_interval_ms)).await;
    }
}

async fn setup_and_poll(config: &Config, client: &redis::Client) -> anyhow::Result<()> {
    info!("Connecting to redis instance: {}", config.redis_url);
    let mut con = client.get_multiplexed_async_connection().await?;
    let job_con = con.clone();

    info!(
        "Setting up redis queue: {}",
        config.redis_new_article_task_queue
    );
    let rsmq = Rsmq::new_with_connection(job_con, false, None);
    let mut producer = RedisJobProducer::new(config.redis_new_article_task_queue.as_str(), rsmq);
    let _ = producer.setup().await; // ignore error, queue could already exist

    info!("Trying to poll feed: {}", config.feed_url);
    poll_feed(config, &mut con, &mut producer).await?;
    Ok(())
}

async fn poll_feed(
    config: &Config,
    article_cache: &mut impl KeyValueCache,
    job_producer: &mut impl JobProducer,
) -> anyhow::Result<()> {
    let response = reqwest::get(&config.feed_url).await?;
    let content = response.bytes().await?;
    let channel = rss::Channel::read_from(&content[..])?;

    for item in channel.items() {
        process_item(config, item, article_cache, job_producer).await?;
    }
    Ok(())
}

async fn process_item(
    config: &Config,
    item: &Item,
    article_cache: &mut impl KeyValueCache,
    job_producer: &mut impl JobProducer,
) -> anyhow::Result<()> {
    match compute_cache_key(item) {
        Ok(item_key) => {
            info!("Processing item {}", &item_key);

            let item_content = article_cache.get(&item_key).await?;
            if item_content.is_none() {
                let item = fetch_item(item, config.max_iframes).await?;
                article_cache
                    .set(
                        &item_key,
                        serde_json::to_string(&item)?.as_str(),
                        Duration::from_secs(config.cache_expire_time_s),
                    )
                    .await?;
                job_producer.produce_job(&item.header).await?;
            }
        }
        Err(e) => {
            info!("Error processing item: {:?}", e);
        }
    }
    Ok(())
}

async fn fetch_item(item: &Item, max_iframes: u8) -> anyhow::Result<gazzetta_common::Item> {
    let header = ItemHeader::new(compute_cache_key(item)?);
    let content = fetch_from_reqwest(item.link(), max_iframes)
        .await
        .map(|corpus| corpus.join("\n"));

    Ok(gazzetta_common::Item {
        header,
        title: item.title().map(|s| s.to_string()),
        pub_date: item.pub_date().map(|s| s.to_string()),
        summary: item.content().map(|s| s.to_string()),
        url: item.link().map(|s| s.to_string()),
        content,
    })
}

async fn fetch_from_reqwest(link: Option<&str>, max_iframe_parsed: u8) -> Option<Vec<String>> {
    let mut links = vec![link?.to_string()];
    let mut bodies = vec![];

    while !links.is_empty() && bodies.len() < max_iframe_parsed as usize {
        let link = links.pop()?;
        let response = reqwest::get(&link).await.ok()?;
        if let Ok(body) = response.text().await {
            let document = scraper::Html::parse_document(&body);
            if let Ok(base_url) = Url::parse(&link) {
                links.extend(
                    document
                        .select(&scraper::Selector::parse("iframe").unwrap())
                        .filter_map(|frame| frame.value().attr("src"))
                        .filter_map(|src| match base_url.join(src) {
                            Ok(url) => Some(url.to_string()),
                            Err(e) => {
                                info!("Error parsing iframe URL: {:?}", e);
                                None
                            }
                        }),
                );
            }
            bodies.push(body);
        }
    }

    Some(bodies)
}

fn compute_cache_key(item: &Item) -> anyhow::Result<String> {
    Ok(item
        .guid()
        .map(|guid| guid.value().to_string())
        .or_else(|| item.link().map(|link| link.to_string()))
        .unwrap_or(format!(
            "({},{},{})",
            item.title().context("Missing title")?,
            item.pub_date().context("Missing pub date")?,
            item.content().context("Missing description")?
        )))
}

trait JobProducer {
    fn setup(&mut self) -> impl Future<Output = anyhow::Result<()>>;
    fn produce_job(&mut self, item: &ItemHeader) -> impl Future<Output = anyhow::Result<()>>;
}

struct RedisJobProducer<'a> {
    queue_name: &'a str,
    rqsm: Rsmq,
}

impl<'a> JobProducer for RedisJobProducer<'a> {
    async fn produce_job(&mut self, item: &ItemHeader) -> anyhow::Result<()> {
        self.rqsm
            .send_message(self.queue_name, serde_json::to_string(item)?, None)
            .await?;
        Ok(())
    }

    async fn setup(&mut self) -> anyhow::Result<()> {
        self.rqsm
            .create_queue(self.queue_name, None, None, None)
            .await?;
        Ok(())
    }
}

impl RedisJobProducer<'_> {
    fn new(queue_name: &str, rsmq: Rsmq) -> RedisJobProducer {
        RedisJobProducer {
            queue_name,
            rqsm: rsmq,
        }
    }
}
