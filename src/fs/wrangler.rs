use crate::fs::{DirectoryType, file::StaticFile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Wrangler {
    account_id: Option<String>,
    worker: String,
}

impl Wrangler {
    pub fn account_id(&self) -> Option<&str> {
        self.account_id.as_deref()
    }

    pub fn worker(&self) -> &str {
        &self.worker
    }
}

pub struct WranglerFile;

impl StaticFile for WranglerFile {
    type Data = Wrangler;
    const DIR: DirectoryType = DirectoryType::Project;
    const NAME: &'static str = "wrangler";
    const EXTENSION: &'static str = "toml";
}
