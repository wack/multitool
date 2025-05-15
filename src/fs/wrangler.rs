use serde::{Deserialize, Serialize};
use crate::fs::{DirectoryType, file::StaticFile};

#[derive(Debug, Serialize, Deserialize)]
pub struct Wrangler {
    account_id: String,
    worker: String,
}

impl Wrangler {
    pub fn account_id(&self) -> &str {
        &self.account_id
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
