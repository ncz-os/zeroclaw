use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::canon::canonicalize;
use crate::schema::{ModelPrice, Source};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(flatten)]
    prices: BTreeMap<String, ModelPrice>,
}

impl Catalog {
    pub fn new(prices: BTreeMap<String, ModelPrice>) -> Self {
        Self { prices }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, CatalogError> {
        Self::from_json_str(&fs::read_to_string(path)?)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), CatalogError> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }

    pub fn from_json_str(s: &str) -> Result<Self, CatalogError> {
        let value: serde_json::Value = serde_json::from_str(s)?;
        Self::from_value(value)
    }

    pub fn from_value(value: serde_json::Value) -> Result<Self, CatalogError> {
        match value {
            serde_json::Value::Array(items) => {
                let mut prices = BTreeMap::new();
                for item in items {
                    let mut price: ModelPrice = serde_json::from_value(item)?;
                    let (_, key) = canonicalize(&price.model_id);
                    price.model_id = key.clone();
                    if price.source != Source::Catalog {
                        price.source = Source::Catalog;
                    }
                    prices.insert(key, price);
                }
                Ok(Self { prices })
            }
            serde_json::Value::Object(_) => {
                let mut catalog: Self = serde_json::from_value(value)?;
                for (key, price) in &mut catalog.prices {
                    let (_, canonical_key) = canonicalize(key);
                    if canonical_key != *key {
                        price.model_id = canonicalize(&price.model_id).1;
                    }
                }
                Ok(catalog)
            }
            _ => Err(CatalogError::InvalidShape),
        }
    }

    pub fn insert(&mut self, price: ModelPrice) {
        self.prices.insert(price.model_id.clone(), price);
    }

    pub fn price_for(&self, model: &str) -> Option<ModelPrice> {
        let (_, model_id) = canonicalize(model);
        self.prices.get(&model_id).cloned().map(|mut price| {
            price.source = Source::Catalog;
            price
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &ModelPrice)> {
        self.prices.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }
}

#[derive(Debug)]
pub enum CatalogError {
    Io(io::Error),
    Json(serde_json::Error),
    InvalidShape,
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "catalog I/O error: {err}"),
            Self::Json(err) => write!(f, "catalog JSON error: {err}"),
            Self::InvalidShape => write!(f, "catalog must be a JSON object or array"),
        }
    }
}

impl std::error::Error for CatalogError {}

impl From<io::Error> for CatalogError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for CatalogError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
