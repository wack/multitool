/// The [Group] indicates from whence a given observation
/// was generated: either by a control group rollout or by
/// a canary rollout.
#[derive(Hash, Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum Group {
    /// The control group is the current running rollout.
    Control,
    /// The experimental group represents the canary rollout.
    Experimental,
}
