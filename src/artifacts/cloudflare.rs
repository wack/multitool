use chrono::{Date, NaiveDate, Utc};
use nutype::nutype;
use serde::{Deserialize, Serialize};

/// CloudFlareWorkerSrc is a snapshot of the files in the filesystem
/// and their SHAs, ready to be uploaded to CF. If the filesystem
/// changes after this struct has been created, it will be
/// invalidated.
pub struct CloudFlareWorkerSrc {}

/// https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/versions/methods/create/
pub struct VersionUploadMetadata {
    main_module: String,
    annotations: Annotations,
    bindings: Option<Vec<Binding>>,
    // TODO: It's not clear to me which data we should pick.
    compatibility_date: Option<NaiveDate>,
    // TODO: It's not clear to me what the string value references.
    // TODO: maybe it should be a nutype
    keep_bindings: Option<Vec<String>>,
}

#[nutype(
    sanitize(trim),
    validate(not_empty, len_char_max = 32),
    derive(Debug, Serialize, Deserialize)
)]
pub struct AccountId(String);

// TODO
pub struct Binding;

#[derive(Deserialize, Serialize)]
pub struct Annotations {
    // Max Length: 100
    #[serde(rename = "workers/message")]
    message: Option<Message>,
    // Max Length: 25
    #[serde(rename = "workers/tag")]
    tag: Option<Tag>,
}

#[nutype(
    sanitize(trim),
    validate(not_empty, len_char_max = 100),
    derive(Debug, Serialize, Deserialize)
)]
pub struct Message(String);

#[nutype(
    sanitize(trim),
    validate(not_empty, len_char_max = 25),
    derive(Debug, Serialize, Deserialize)
)]
pub struct Tag(String);
