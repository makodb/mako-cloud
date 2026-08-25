#![forbid(unsafe_code)]

//! Plans, what they include, and the limits that follow from them.
//!
//! A plan decides what a customer may use. Two different things get called a
//! "limit" and conflating them is the mistake this module exists to avoid:
//!
//! - An **included amount** is what the price covers. Going past it is normal
//!   on a paid plan: the customer keeps working and the excess is billed as
//!   overage. Going past it on a free plan is not, because there is nobody to
//!   bill, so the free plan caps instead.
//! - A **cap** is a refusal. The request does not happen.
//!
//! Rate limits are a third thing again and belong to neither: they protect the
//! platform from a burst regardless of what anyone has paid, so every plan
//! carries them and no amount of money removes them.

pub mod rating;

use std::collections::BTreeMap;

use mako_api::QuotaResource;
use mako_gateway::{
    GatewayQuotaLimit, GatewayQuotaPolicy, GatewayQuotaPolicyError, GatewayQuotaResource,
    GatewayQuotaWindow,
};
use serde::{Deserialize, Serialize};

/// What a plan grants for one resource.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Entitlement {
    /// What the price covers for a billing period.
    pub included: u64,
    /// Whether exceeding the included amount is billed rather than refused.
    ///
    /// A plan with no way to bill anyone cannot allow this, which is what makes
    /// the free tier cap where a paid tier keeps serving.
    pub overage_billed: bool,
}

impl Entitlement {
    /// How much of this period's use is chargeable beyond the plan.
    ///
    /// Zero when the use is inside what the plan includes, and zero when the
    /// plan refuses overage, because then the use never happened.
    #[must_use]
    pub fn overage(&self, used: u64) -> u64 {
        if !self.overage_billed {
            return 0;
        }
        used.saturating_sub(self.included)
    }

    /// The cap this entitlement implies, if any.
    #[must_use]
    pub fn cap(&self) -> Option<u64> {
        (!self.overage_billed).then_some(self.included)
    }
}

/// A named set of entitlements, priced elsewhere.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Plan {
    pub id: String,
    pub display_name: String,
    /// Bumped when the terms change, so a closed period can be re-derived
    /// against the terms that applied while it was open.
    pub version: u64,
    pub entitlements: BTreeMap<QuotaResource, Entitlement>,
}

impl Plan {
    #[must_use]
    pub fn entitlement(&self, resource: QuotaResource) -> Option<Entitlement> {
        self.entitlements.get(&resource).copied()
    }

    /// What this plan's terms mean for one period's measured use.
    #[must_use]
    pub fn overage(&self, resource: QuotaResource, used: u64) -> u64 {
        self.entitlement(resource)
            .map_or(0, |entitlement| entitlement.overage(used))
    }
}

/// An operator's decision to ignore the plan for one resource.
///
/// Deliberately narrow. It names one resource, carries a reason, and may
/// expire, because "this customer is special" without a stated reason and an
/// end date is how a deployment stops knowing what anyone is entitled to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlanException {
    pub resource: QuotaResource,
    pub included: u64,
    pub overage_billed: bool,
    pub reason: String,
    pub expires_at_unix_seconds: Option<u64>,
}

impl PlanException {
    #[must_use]
    pub fn applies_at(&self, now_unix_seconds: u64) -> bool {
        self.expires_at_unix_seconds
            .is_none_or(|expires| expires > now_unix_seconds)
    }

    #[must_use]
    pub fn entitlement(&self) -> Entitlement {
        Entitlement {
            included: self.included,
            overage_billed: self.overage_billed,
        }
    }
}

/// What a customer is entitled to right now: their plan, with any exception
/// an operator has recorded applied on top.
#[must_use]
pub fn effective_entitlements(
    plan: &Plan,
    exceptions: &[PlanException],
    now_unix_seconds: u64,
) -> BTreeMap<QuotaResource, Entitlement> {
    let mut effective = plan.entitlements.clone();
    for exception in exceptions {
        if exception.applies_at(now_unix_seconds) {
            effective.insert(exception.resource, exception.entitlement());
        }
    }
    effective
}

/// The plan as it applies to one organization right now: its subscribed plan
/// with any operator-recorded exception laid over it. Everything downstream --
/// enforcement and rating alike -- consumes the result, so an exception
/// changes the limits and the bill together rather than one drifting from the
/// other.
#[must_use]
pub fn effective_plan(plan: &Plan, exceptions: &[PlanException], now_unix_seconds: u64) -> Plan {
    Plan {
        entitlements: effective_entitlements(plan, exceptions, now_unix_seconds),
        ..plan.clone()
    }
}

/// Every plan carries these regardless of price: they exist to keep one
/// customer's burst from becoming everyone's outage.
#[must_use]
pub fn platform_rate_limits() -> Vec<(GatewayQuotaResource, GatewayQuotaWindow)> {
    const MINUTE: u64 = 60_000;
    vec![
        (
            GatewayQuotaResource::AuthenticationRequests,
            window(600, MINUTE),
        ),
        (
            GatewayQuotaResource::DocumentRequests,
            window(10_000, MINUTE),
        ),
        (
            GatewayQuotaResource::DocumentBytes,
            window(50 * 1024 * 1024, MINUTE),
        ),
        (
            GatewayQuotaResource::ReplicationRequests,
            window(120, MINUTE),
        ),
        (
            GatewayQuotaResource::ReplicationBytes,
            window(64 * 1024 * 1024, MINUTE),
        ),
    ]
}

/// Turn what a customer is entitled to into what the gateway enforces.
///
/// Only entitlements that refuse overage become caps. An entitlement that bills
/// for overage must not cap, or the customer would be blocked at exactly the
/// point they started paying for more -- which is the opposite of what they
/// bought.
pub fn enforcement_policy(
    entitlements: &BTreeMap<QuotaResource, Entitlement>,
) -> Result<GatewayQuotaPolicy, GatewayQuotaPolicyError> {
    const MONTH: u64 = 30 * 24 * 60 * 60 * 1_000;
    let mut limits: BTreeMap<GatewayQuotaResource, GatewayQuotaLimit> = platform_rate_limits()
        .into_iter()
        .map(|(resource, rate)| {
            (
                resource,
                GatewayQuotaLimit {
                    hard: None,
                    rate: Some(rate),
                },
            )
        })
        .collect();

    for (resource, entitlement) in entitlements {
        let Some(cap) = entitlement.cap() else {
            continue;
        };
        // Only the resources the gateway actually meters can be capped by it.
        // Stored bytes and user counts are levels measured on a schedule, not
        // something a single request can be refused for here.
        let enforced = match resource {
            QuotaResource::ReplicationRequestsPerMinute => {
                Some((GatewayQuotaResource::ReplicationRequests, MONTH))
            }
            QuotaResource::ReplicationBytesPerMonth => {
                Some((GatewayQuotaResource::ReplicationBytes, MONTH))
            }
            _ => None,
        };
        let Some((gateway_resource, window_milliseconds)) = enforced else {
            continue;
        };
        let entry = limits.entry(gateway_resource).or_default();
        entry.hard = Some(window(cap.max(1), window_milliseconds));
    }

    GatewayQuotaPolicy::new(limits).map_err(|_| GatewayQuotaPolicyError::Unavailable)
}

fn window(limit: u64, milliseconds: u64) -> GatewayQuotaWindow {
    GatewayQuotaWindow {
        limit: limit.max(1).try_into().expect("limit is at least one"),
        window_milliseconds: milliseconds
            .max(1)
            .try_into()
            .expect("window is at least one"),
    }
}

/// The plans this deployment offers.
///
/// Shaped after the tiering the pricing decision named: a free tier that caps,
/// and a paid tier that keeps serving and bills the excess. The numbers are
/// deliberately here as data and not thresholds scattered through the code, so
/// changing what a plan includes is an edit to one list.
///
/// Prices live with the rate card, not here. A plan is what a customer may use;
/// what it costs is a separate decision and can still be zero.
#[must_use]
pub fn catalog() -> Vec<Plan> {
    vec![
        Plan {
            id: "free".to_owned(),
            display_name: "Free".to_owned(),
            version: 1,
            entitlements: BTreeMap::from([
                (
                    QuotaResource::StorageBytes,
                    Entitlement {
                        included: 500 * 1024 * 1024,
                        overage_billed: false,
                    },
                ),
                (
                    QuotaResource::ReplicationBytesPerMonth,
                    Entitlement {
                        included: 5 * 1024 * 1024 * 1024,
                        overage_billed: false,
                    },
                ),
                (
                    QuotaResource::ApplicationUsers,
                    Entitlement {
                        included: 50_000,
                        overage_billed: false,
                    },
                ),
                (
                    QuotaResource::EdgeInvocationsPerMonth,
                    Entitlement {
                        included: 500_000,
                        overage_billed: false,
                    },
                ),
            ]),
        },
        Plan {
            id: "pro".to_owned(),
            display_name: "Pro".to_owned(),
            version: 1,
            entitlements: BTreeMap::from([
                (
                    QuotaResource::StorageBytes,
                    Entitlement {
                        included: 8 * 1024 * 1024 * 1024,
                        overage_billed: true,
                    },
                ),
                (
                    QuotaResource::ReplicationBytesPerMonth,
                    Entitlement {
                        included: 250 * 1024 * 1024 * 1024,
                        overage_billed: true,
                    },
                ),
                (
                    QuotaResource::ApplicationUsers,
                    Entitlement {
                        included: 100_000,
                        overage_billed: true,
                    },
                ),
                (
                    QuotaResource::EdgeInvocationsPerMonth,
                    Entitlement {
                        included: 2_000_000,
                        overage_billed: true,
                    },
                ),
            ]),
        },
    ]
}

#[must_use]
pub fn plan(id: &str) -> Option<Plan> {
    catalog().into_iter().find(|plan| plan.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free() -> Plan {
        plan("free").expect("free plan")
    }

    fn pro() -> Plan {
        plan("pro").expect("pro plan")
    }

    #[test]
    fn a_paid_plan_bills_the_excess_and_a_free_plan_refuses_it() {
        let resource = QuotaResource::StorageBytes;
        let free_included = free().entitlement(resource).expect("entitlement").included;
        let pro_included = pro().entitlement(resource).expect("entitlement").included;

        // Going past what you pay for is the normal case on a paid plan.
        assert_eq!(pro().overage(resource, pro_included + 1_000), 1_000);
        assert_eq!(pro().overage(resource, pro_included), 0);

        // On a free plan there is nobody to bill, so there is no overage to
        // report -- the use is refused instead, which is what the cap says.
        assert_eq!(free().overage(resource, free_included + 1_000), 0);
        assert_eq!(
            free().entitlement(resource).expect("entitlement").cap(),
            Some(free_included)
        );
        assert_eq!(
            pro().entitlement(resource).expect("entitlement").cap(),
            None
        );
    }

    #[test]
    fn a_paid_plan_is_never_capped_at_the_point_it_starts_charging() {
        let entitlements = pro().entitlements;
        let policy = enforcement_policy(&entitlements).expect("policy");

        // The whole promise of overage is that the customer keeps working. A
        // cap at the included amount would stop them at exactly the moment
        // they began paying for more.
        let replication = policy
            .limit(GatewayQuotaResource::ReplicationBytes)
            .expect("replication bytes are limited");
        assert!(
            replication.hard.is_none(),
            "a plan that bills for overage capped the resource anyway"
        );
        assert!(
            replication.rate.is_some(),
            "a paid plan dropped the rate limit that protects the platform"
        );
    }

    #[test]
    fn a_free_plan_is_capped_but_still_rate_limited() {
        let policy = enforcement_policy(&free().entitlements).expect("policy");
        let replication = policy
            .limit(GatewayQuotaResource::ReplicationBytes)
            .expect("replication bytes are limited");
        assert!(
            replication.hard.is_some(),
            "a plan that cannot bill for overage did not cap"
        );
        assert!(replication.rate.is_some());
    }

    #[test]
    fn an_exception_overrides_the_plan_and_stops_when_it_expires() {
        let resource = QuotaResource::ReplicationBytesPerMonth;
        let exception = PlanException {
            resource,
            included: 999,
            overage_billed: true,
            reason: "raised for a launch".to_owned(),
            expires_at_unix_seconds: Some(2_000),
        };

        let during = effective_entitlements(&free(), std::slice::from_ref(&exception), 1_000);
        assert_eq!(during[&resource].included, 999);
        assert!(
            during[&resource].overage_billed,
            "an exception could not lift a free plan's refusal to allow overage"
        );

        // An exception that has run out leaves the customer on their plan, so
        // a forgotten override cannot quietly become permanent.
        let after = effective_entitlements(&free(), std::slice::from_ref(&exception), 3_000);
        assert_eq!(
            after[&resource],
            free().entitlement(resource).expect("entitlement"),
            "an expired exception was still being applied"
        );
    }

    /// The control plane resolves a plan into limits and sends them as JSON;
    /// the data plane parses them back. Nothing else checks that those two
    /// agree, and a mismatch would not be loud: the gateway would simply fail
    /// to read the policy it was given.
    #[test]
    fn resolved_limits_survive_the_trip_between_the_planes() {
        for plan in catalog() {
            let resolved = enforcement_policy(&plan.entitlements).expect("policy");
            let sent = serde_json::to_vec(&resolved).expect("serialize");
            let received: GatewayQuotaPolicy = serde_json::from_slice(&sent).expect("deserialize");
            assert_eq!(
                received, resolved,
                "the {} plan's limits did not survive being sent",
                plan.id
            );
        }
    }

    #[test]
    fn a_resource_the_gateway_cannot_refuse_is_not_pretended_to_be_capped() {
        // Stored bytes is measured on a schedule, not decided per request, so
        // a cap on it cannot be honoured here and must not be advertised.
        let policy = enforcement_policy(&free().entitlements).expect("policy");
        assert!(
            policy.limit(GatewayQuotaResource::DocumentBytes).is_some(),
            "the platform rate limit went missing"
        );
        assert_eq!(
            policy
                .limit(GatewayQuotaResource::DocumentBytes)
                .expect("limit")
                .hard,
            None
        );
    }
}
