use openapi::models::LoginSuccess;
use serde::{Deserialize, Serialize};

use super::{DirectoryType, StaticFile};

/// This file contains the user's session info, stored as JSON.
/// This session info is created when the user logs in with
/// `wack login` and deleted when the user runs `wack logout`.
/// The session information is used to make authenticated HTTP requests
/// to the registry webserver.
#[derive(Serialize, Deserialize)]
pub enum Session {
    User(UserCreds),
}

/// A `UserToken` contains the login credentials for a human-operator
/// session.
#[derive(Serialize, Deserialize)]
pub struct UserCreds {
    /// The email of the user who logged in.
    email: String,
    /// The user's JWT, which is required for making HTTP requests
    /// to certain backend routes.
    jwt: String,
    // TODO(@RM): Add an expiration time to the result of login.
    // expiry: chrono::Datetime<Utc>
}

/// Add a convertion from the response type into our internal type.
impl From<LoginSuccess> for UserCreds {
    fn from(login: LoginSuccess) -> Self {
        UserCreds {
            email: login.email,
            jwt: login.jwt,
        }
    }
}

/*
impl From<UserCreateSuccess> for UserCreds {
    fn from(user_create: UserCreateSuccess) -> Self {
        UserCreds {
            email: user_create.email,
            jwt: user_create.jwt,
        }
    }
}
*/

impl StaticFile for Session {
    /// Session information is by nature ephemeral. It can be safely
    /// deleted. That's why its considered cache.
    const DIR: DirectoryType = DirectoryType::Cache;
    const NAME: &'static str = "session";
    const EXTENSION: &'static str = "json";

    type Data = Self;
}
