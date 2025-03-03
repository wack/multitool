use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{DirectoryType, StaticFile};

/// This file contains the user's session info, stored as JSON.
/// This session info is created when the user logs in with
/// `wack login` and deleted when the user runs `wack logout`.
/// The session information is used to make authenticated HTTP requests
/// to the registry webserver.
#[derive(Serialize, Deserialize, Clone)]
pub enum Session {
    User(UserCreds),
}

impl Session {
    pub fn is_not_expired(creds: UserCreds) -> Self {
        if creds.expiry >= Utc::now() {
            Self::User(creds)
        } else {
            panic!("Login token expired, please login again with \'multitool login\'.");
        }
    }
}

/// A `UserToken` contains the login credentials for a human-operator
/// session.
#[derive(Serialize, Deserialize, Clone)]
pub struct UserCreds {
    /// The email of the user who logged in.
    pub email: String,
    /// The user's JWT, which is required for making HTTP requests
    /// to certain backend routes.
    pub jwt: String,
    /// The expiry date of the JWT.
    pub expiry: DateTime<Utc>,
}

impl UserCreds {
    pub fn new(email: String, jwt: String, expiry: DateTime<Utc>) -> Self {
        Self { email, jwt, expiry }
    }
}

pub struct SessionFile;

impl StaticFile for SessionFile {
    /// Session information is by nature ephemeral. It can be safely
    /// deleted. That's why its considered cache.
    const DIR: DirectoryType = DirectoryType::Cache;
    const NAME: &'static str = "session";
    const EXTENSION: &'static str = "json";
    type Data = Session;
}
