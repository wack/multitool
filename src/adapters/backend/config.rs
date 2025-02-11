use std::ops::Deref;

use openapi::apis::configuration::Configuration;

use crate::Flags;

pub struct BackendConfig {
    conf: Configuration,
}

impl From<&Flags> for BackendConfig {
    fn from(flags: &Flags) -> Self {
        Self::new(flags.origin())
    }
}

impl BackendConfig {
    pub fn new<T: AsRef<str>>(origin: Option<T>) -> Self {
        // • Convert the Option<T> to a String.
        let origin = origin.map(|val| val.as_ref().to_owned());
        // • Set up the default configuration values.
        let mut conf = Configuration {
            ..Configuration::default()
        };
        // • Override the default origin.
        if let Some(origin) = origin {
            conf.base_path = origin;
        }
        Self { conf }
    }
}

impl Deref for BackendConfig {
    type Target = Configuration;

    fn deref(&self) -> &Self::Target {
        &self.conf
    }
}
