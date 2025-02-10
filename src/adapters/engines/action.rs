use crate::WholePercent;

/// An [Action] describes an effectful operation affecting the deployments.
/// Actions describe decisions made by the [DecisionEngine].
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    /// Ramp the canary to 100% traffic and decommission the baseline deployment.
    Promote,
    /// Ramp the baseline to 100% traffic and decommission the canary deployment.
    Rollback,
    /// RampUp indicates the amount of traffic provided to the canary should increase
    /// by one unit.
    RampTo(WholePercent),
    // NB: We don't have a no-op action type, which might be something the DecisionEngine
    //     provides, except that I'm picturing this Action type as part of the interface
    //     into the Ingress, so the Ingress just won't hear back anything from the engine
    //     if that's the case.
}
