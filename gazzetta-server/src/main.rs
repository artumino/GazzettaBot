// Ideally this would use a relational DB to figure out who and what we need to notify
// Right now this is hardcoded to what i need

use std::{cell::OnceCell, collections::HashSet, time::Duration};

use aho_corasick::PatternID;
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
    info!(
        "Consumer waiting for new items in queue: {}",
        config.new_article_task_queue
    );
    loop {
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
            let matched_words = find_matches(config, &item);

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

fn find_matches(config: &Config, item: &Item) -> Vec<String> {
    let search_keywords = OnceCell::new();
    let search_keywords = search_keywords.get_or_init(|| {
        config
            .search_keywords
            .split(',')
            .map(|word| word.to_lowercase())
            .collect::<Vec<String>>()
    });
    let search_automaton = OnceCell::new();
    let search_automaton = search_automaton.get_or_init(|| {
         aho_corasick::AhoCorasick::builder()
        .ascii_case_insensitive(true)
        .build(search_keywords)
        .unwrap()
    });

    let mut matched_patterns = HashSet::new();
    add_matches(&item.title, search_automaton, &mut matched_patterns);
    add_matches(&item.summary, search_automaton, &mut matched_patterns);
    add_matches(&item.content, search_automaton, &mut matched_patterns);

    let matched_words: Vec<String> = matched_patterns
        .iter()
        .map(|pattern_id| search_keywords[*pattern_id].to_string())
        .collect();
    matched_words
}

fn add_matches(corpus: &Option<String>, search_automaton: &aho_corasick::AhoCorasick, matched_patterns: &mut HashSet<PatternID>) {
    if let Some(ref corpus) = corpus {
        for mat in search_automaton.find_iter(corpus) {
            matched_patterns.insert(mat.pattern());
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_find_matches() {
        let config = Config {
            search_keywords: "foo,bar,quick brown fox,quick green fox,Dls.G/,gree".to_string(),
            redis_url: "redis://localhost:6379".to_string(),
            new_article_task_queue: "new_article_task_queue".to_string(),
            notify_task_queue: "notify_task_queue".to_string(),
        };
        let item = Item {
            title: Some("foo bar Dls.G/".to_string()),
            summary: Some("foo bar and the quIck brown fox".to_string()),
            content: Some("foo bar and the quick foo brown fox".to_string()),
            header: ItemHeader {
                key: "key".to_string()
            },
            pub_date: Some("".to_string()),
            url: Some("http://example.com".to_string()),
        };

        // foo, bar, quick brown fox, Dls.G/
        let mut matched_words = find_matches(&config, &item);
        matched_words.sort();
        let mut expected = vec!["foo", "bar", "quick brown fox", "dls.g/"];
        expected.sort();
        assert_eq!(matched_words, expected);
    }
}