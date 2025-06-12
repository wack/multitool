use crate::fs::{DirectoryType, file::StaticFile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Wrangler {
    account_id: Option<String>,
    name: String,
    main: String,
}

impl Wrangler {
    pub fn account_id(&self) -> Option<&str> {
        self.account_id.as_deref()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn main(&self) -> &str {
        &self.main
    }
}

pub struct WranglerFile;

impl StaticFile for WranglerFile {
    type Data = Wrangler;
    const DIR: DirectoryType = DirectoryType::ApplicationRoot;
    const NAME: &'static str = "wrangler";
    const EXTENSION: &'static str = "toml";
}
