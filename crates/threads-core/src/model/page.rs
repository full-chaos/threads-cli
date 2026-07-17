use serde::{Deserialize, Serialize};

use super::Cursor;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<Cursor>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, next: Option<Cursor>) -> Self {
        Self { items, next }
    }

    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            next: None,
        }
    }
}
