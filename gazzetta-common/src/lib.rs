use serde::{Deserialize, Serialize};

pub mod cache;

#[derive(Serialize, Deserialize)]
pub struct ItemHeader {
    pub key: String,
}

impl ItemHeader {
    pub fn new(key: String) -> ItemHeader {
        ItemHeader { key }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Item {
    #[serde(flatten)]
    pub header: ItemHeader,
    pub title: Option<String>,
    pub pub_date: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub url: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct MatchResult {
    pub matched_words: Vec<String>,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub url: Option<String>,
}
