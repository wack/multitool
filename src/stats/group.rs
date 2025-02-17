/// The [Group] indicates from whence a given observation
/// was generated: either by a control group deployment or by
/// a canary deployment.
#[derive(Hash, Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum Group {
    /// The control group is the current running deployment.
    Control,
    /// The experimental group represents the canary deployment.
    Experimental,
}
