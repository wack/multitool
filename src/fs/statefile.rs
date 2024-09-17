use serde::{Deserialize, Serialize};

use crate::fs::DirectoryType;

use super::file::MultiFileInstance;

#[derive(Serialize, Deserialize)]
pub struct Statefile {
    project_name: String,
}

/// A manifest that was originally loaded from a TOML file.
impl MultiFileInstance for Statefile {
    const DIR: DirectoryType = DirectoryType::Cache;
    const EXTENSION: &'static str = "json";

    fn name(&self) -> String {
        format!("{}-statefile", &self.project_name)
    }
}
