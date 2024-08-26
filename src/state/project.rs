use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    /// Every project must be uniquely identifiable with
    /// a name, at least for the purposes of local development.
    /// We want to store state in the $XDG_DATA directory so users
    /// can't accidently hose their statefile. But to do so, each
    /// project on a given machine has to have a unique ID in its
    /// configuration so you have a way to find the state file
    /// using only the data available in the current directory.
    name: String,
}
