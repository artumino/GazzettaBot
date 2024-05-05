use serde::{Deserialize, Serialize};

pub mod cache;

#[derive(Serialize, Deserialize)]
pub struct ItemHeader {
    pub title: String,
    pub pub_date: String,
}

impl ItemHeader {
    pub fn new(title: String, pub_date: String) -> ItemHeader {
        ItemHeader { title, pub_date }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Item {
    #[serde(flatten)]
    pub header: ItemHeader,
    pub summary: Option<String>,
    pub content: Option<String>,
}

impl ItemHeader {
    pub fn cache_key(&self) -> String {
        format!("({},{})", self.title, self.pub_date)
    }
}
