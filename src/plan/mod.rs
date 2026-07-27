//! Single private owner of the immutable logical-call plan.

mod contract;
mod policy;

pub(crate) use contract::{
    CallExecutionIntent, NormalizationReport, PlanProvenance, PlannedRequest, ResolvedCallPlan,
};
pub(crate) use policy::{
    CallPolicySnapshot, ProtocolKind, ResolvedLimits, ResolvedTarget, ResponseLimits,
};
