//! Turn a period's measured use into the bill it implies.
//!
//! Money here is micro-dollars in integers. Prices like a third of a cent per
//! user cannot be represented in cents, and floats drift -- an invoice that
//! re-derives to a different total than it was issued with is the one bug this
//! module must never have.

use std::collections::BTreeMap;

use mako_api::QuotaResource;
use serde::{Deserialize, Serialize};

use crate::Plan;

/// Micro-dollars: one millionth of a dollar.
pub type MicroDollars = i64;

/// How a resource's samples become one quantity for the period.
///
/// A flow is a stream of increments -- bytes served, invocations admitted --
/// and summing its records is the period's total. A level is a height --
/// bytes stored, users existing -- and its records are samples of the same
/// underlying number, so summing them would bill a tenant once per sample for
/// the same stored data. Levels average instead, which is also what makes a
/// mid-period delete fair: half a month at full size is half the charge.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    SumOfRecords,
    AverageOfSamples,
}

#[must_use]
pub fn aggregation(resource: QuotaResource) -> Aggregation {
    match resource {
        QuotaResource::StorageBytes
        | QuotaResource::ObjectStorageBytes
        | QuotaResource::ApplicationUsers
        | QuotaResource::Environments
        | QuotaResource::CollectionsPerEnvironment
        | QuotaResource::EdgeFunctions => Aggregation::AverageOfSamples,
        QuotaResource::ReplicationRequestsPerMinute
        | QuotaResource::ReplicationBytesPerMonth
        | QuotaResource::ObjectEgressBytesPerMonth
        | QuotaResource::EdgeInvocationsPerMonth
        | QuotaResource::EdgeComputeMillisecondsPerMonth
        | QuotaResource::LogBytesPerMonth => Aggregation::SumOfRecords,
    }
}

/// Running sum and sample count, independent of the number of usage records.
#[derive(Clone, Copy, Debug, Default)]
pub struct PeriodQuantity {
    total: u128,
    samples: u64,
}

impl PeriodQuantity {
    pub fn record(&mut self, quantity: u64) {
        self.total = self.total.saturating_add(u128::from(quantity));
        self.samples = self.samples.saturating_add(1);
    }

    #[must_use]
    pub fn quantity(self, resource: QuotaResource) -> u64 {
        let quantity = match aggregation(resource) {
            Aggregation::SumOfRecords => self.total,
            Aggregation::AverageOfSamples => self.total / u128::from(self.samples.max(1)),
        };
        u64::try_from(quantity).unwrap_or(u64::MAX)
    }
}

/// Reduce a period's usage records for one resource to the quantity billed.
#[must_use]
pub fn period_quantity(resource: QuotaResource, records: &[u64]) -> u64 {
    let mut total = PeriodQuantity::default();
    for quantity in records {
        total.record(*quantity);
    }
    total.quantity(resource)
}

/// What one unit of overage costs, and what a period on the plan costs before
/// any use.
///
/// Versioned, because a closed period must re-derive against the prices that
/// applied while it was open, not the prices of the day someone re-derives it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RateCard {
    pub version: u64,
    /// Base price per period, by plan id.
    pub base_micro_dollars: BTreeMap<String, MicroDollars>,
    /// Price per unit of overage, by resource. `unit_size` is how many units
    /// one price covers, so per-gigabyte prices do not force per-byte rates.
    pub overage: BTreeMap<QuotaResource, OverageRate>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OverageRate {
    pub micro_dollars: MicroDollars,
    pub unit_size: u64,
}

impl OverageRate {
    /// Charge for a quantity of overage, rounded down to whole priced units.
    ///
    /// Down, not up: the tenant gets the partial unit free rather than paying
    /// for capacity they did not use.
    #[must_use]
    pub fn charge(&self, overage: u64) -> MicroDollars {
        if self.unit_size == 0 {
            return 0;
        }
        let units = overage / self.unit_size;
        i64::try_from(units)
            .unwrap_or(i64::MAX)
            .saturating_mul(self.micro_dollars)
    }
}

/// The default rate card, shaped after Supabase's published pricing.
///
/// Verified against supabase.com/pricing on 2026-08-25: Pro at $25/month,
/// storage $0.125/GB beyond 8 GB, egress $0.09/GB beyond 250 GB, $0.00325 per
/// monthly active user beyond 100,000, $2 per million invocations beyond two
/// million, file storage $0.021/GB beyond 100 GB (priced here at $0.02 per
/// GiB-month beyond 50 GiB), and file egress at the same $0.09/GB as
/// replication. Until the beta ends nothing is collected regardless.
#[must_use]
pub fn default_rate_card() -> RateCard {
    const GIB: u64 = 1024 * 1024 * 1024;
    RateCard {
        version: 2,
        base_micro_dollars: BTreeMap::from([
            ("free".to_owned(), 0),
            // $25 per month.
            ("pro".to_owned(), 25_000_000),
        ]),
        overage: BTreeMap::from([
            (
                QuotaResource::StorageBytes,
                // $0.125 per GiB-month beyond the included amount.
                OverageRate {
                    micro_dollars: 125_000,
                    unit_size: GIB,
                },
            ),
            (
                QuotaResource::ReplicationBytesPerMonth,
                // $0.09 per GiB of bandwidth beyond the included amount.
                OverageRate {
                    micro_dollars: 90_000,
                    unit_size: GIB,
                },
            ),
            (
                QuotaResource::ApplicationUsers,
                // $3.25 per 1,000 monthly active users beyond the included.
                OverageRate {
                    micro_dollars: 3_250_000,
                    unit_size: 1_000,
                },
            ),
            (
                QuotaResource::EdgeInvocationsPerMonth,
                // $2 per million invocations beyond the included amount.
                OverageRate {
                    micro_dollars: 2_000_000,
                    unit_size: 1_000_000,
                },
            ),
            (
                QuotaResource::ObjectStorageBytes,
                // $0.02 per GiB-month of stored objects beyond the included.
                OverageRate {
                    micro_dollars: 20_000,
                    unit_size: GIB,
                },
            ),
            (
                QuotaResource::ObjectEgressBytesPerMonth,
                // $0.09 per GiB of object downloads beyond the included amount.
                OverageRate {
                    micro_dollars: 90_000,
                    unit_size: GIB,
                },
            ),
        ]),
    }
}

/// One resource's contribution to a period's bill, carrying the arithmetic
/// that produced it so the amount is explainable rather than asserted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LineItem {
    pub resource: QuotaResource,
    pub quantity: u64,
    pub included: u64,
    pub overage: u64,
    pub amount_micro_dollars: MicroDollars,
}

/// A period rated against a plan and a rate card.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RatedPeriod {
    pub plan_id: String,
    pub plan_version: u64,
    pub rate_card_version: u64,
    pub base_micro_dollars: MicroDollars,
    pub line_items: Vec<LineItem>,
    pub total_micro_dollars: MicroDollars,
}

/// A project's measured quantity and share of the owner's usage charge.
/// The owner's base subscription and credits are deliberately not allocated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectCostLine {
    pub resource: QuotaResource,
    pub quantity: u64,
    pub amount_micro_dollars: MicroDollars,
}

/// Attribute each resource's rated charge in proportion to project quantities.
/// Remaining-weight integer shares conserve every micro-dollar, with ties resolved
/// by project id. Zero-use projects receive no share, including rounding.
#[must_use]
pub fn allocate_project_costs(
    rated: &RatedPeriod,
    quantities: &BTreeMap<String, BTreeMap<QuotaResource, u64>>,
) -> BTreeMap<String, Vec<ProjectCostLine>> {
    let mut costs: BTreeMap<String, Vec<ProjectCostLine>> = quantities
        .keys()
        .map(|id| (id.clone(), Vec::new()))
        .collect();
    for line in &rated.line_items {
        let total: u128 = quantities
            .values()
            .map(|usage| u128::from(usage.get(&line.resource).copied().unwrap_or(0)))
            .sum();
        let mut remaining_quantity = total;
        let mut remaining_amount = line.amount_micro_dollars.max(0);
        for (id, usage) in quantities {
            let quantity = usage.get(&line.resource).copied().unwrap_or(0);
            // i64 money times u64 quantity fits u128, even when summed
            // project quantities exceed u64::MAX. No usage means no share.
            let remaining = u128::try_from(remaining_amount).expect("nonnegative charge");
            let amount = i64::try_from(
                (remaining * u128::from(quantity))
                    .checked_div(remaining_quantity)
                    .unwrap_or(0),
            )
            .expect("share is bounded by the remaining charge");
            costs
                .get_mut(id)
                .expect("project cost entry")
                .push(ProjectCostLine {
                    resource: line.resource,
                    quantity,
                    amount_micro_dollars: amount,
                });
            remaining_amount -= amount;
            remaining_quantity -= u128::from(quantity);
        }
    }
    costs
}

/// Rate one period's aggregated use.
///
/// Pure: the same plan, card, and use always produce the same bill, which is
/// what lets a finalized invoice be re-derived from retained evidence.
#[must_use]
pub fn rate_period(
    plan: &Plan,
    card: &RateCard,
    usage: &BTreeMap<QuotaResource, u64>,
) -> RatedPeriod {
    let base = card.base_micro_dollars.get(&plan.id).copied().unwrap_or(0);
    let mut line_items = Vec::new();
    let mut total = base;
    for (resource, entitlement) in &plan.entitlements {
        let quantity = usage.get(resource).copied().unwrap_or(0);
        let overage = entitlement.overage(quantity);
        let amount = card
            .overage
            .get(resource)
            .map_or(0, |rate| rate.charge(overage));
        total = total.saturating_add(amount);
        line_items.push(LineItem {
            resource: *resource,
            quantity,
            included: entitlement.included,
            overage,
            amount_micro_dollars: amount,
        });
    }
    RatedPeriod {
        plan_id: plan.id.clone(),
        plan_version: plan.version,
        rate_card_version: card.version,
        base_micro_dollars: base,
        line_items,
        total_micro_dollars: total,
    }
}

/// One stretch of a period during which a single plan applied, with the usage
/// measured inside that stretch. `usage` carries `period_quantity` values
/// computed from the records whose timestamps fell in the stretch.
#[derive(Clone, Debug)]
pub struct PlanSegment {
    pub plan: Plan,
    pub milliseconds: u64,
    pub usage: BTreeMap<QuotaResource, u64>,
}

/// Rate a period whose plan changed partway through.
///
/// Each stretch is rated under its own plan's terms, prorated by how long it
/// held. Flows compare a stretch's total against the stretch's share of the
/// included amount; levels compare a stretch's average against the full
/// included level -- a level is a height, and half a month at nine stored
/// gigabytes is half a month's charge, not a comparison against half the
/// allowance -- and the resulting charge takes the stretch's share of the
/// period. The base fee takes each plan's share of the period too. One
/// segment covering the whole period rates exactly like [`rate_period`].
///
/// Answers `None` when there are no segments or no time: a period of nothing
/// has no plan to rate under, and inventing one would misstate the bill.
#[must_use]
pub fn rate_period_prorated(card: &RateCard, segments: &[PlanSegment]) -> Option<RatedPeriod> {
    let period_milliseconds: u64 = segments.iter().fold(0, |total, segment| {
        total.saturating_add(segment.milliseconds)
    });
    let current = segments.last()?;
    if period_milliseconds == 0 {
        return None;
    }

    let base = segments.iter().fold(0_i64, |total, segment| {
        let plan_base = card
            .base_micro_dollars
            .get(&segment.plan.id)
            .copied()
            .unwrap_or(0);
        total.saturating_add(prorate_amount(
            plan_base,
            segment.milliseconds,
            period_milliseconds,
        ))
    });

    let mut resources = std::collections::BTreeSet::new();
    for segment in segments {
        resources.extend(segment.plan.entitlements.keys().copied());
    }

    let mut line_items = Vec::new();
    let mut total = base;
    for resource in resources {
        let rate = card.overage.get(&resource);
        let mut quantity_weighted: u128 = 0;
        let mut quantity_summed: u64 = 0;
        let mut included_weighted: u128 = 0;
        let mut overage_flow: u64 = 0;
        let mut overage_weighted: u128 = 0;
        let mut amount: MicroDollars = 0;
        for segment in segments {
            let used = segment.usage.get(&resource).copied().unwrap_or(0);
            let entitlement = segment.plan.entitlement(resource);
            let included = entitlement.map_or(0, |entitlement| entitlement.included);
            let billed = entitlement.is_some_and(|entitlement| entitlement.overage_billed);
            match aggregation(resource) {
                Aggregation::SumOfRecords => {
                    let included_share =
                        prorate_quantity(included, segment.milliseconds, period_milliseconds);
                    let overage = if billed {
                        used.saturating_sub(included_share)
                    } else {
                        0
                    };
                    quantity_summed = quantity_summed.saturating_add(used);
                    included_weighted += u128::from(included) * u128::from(segment.milliseconds);
                    overage_flow = overage_flow.saturating_add(overage);
                    amount = amount.saturating_add(rate.map_or(0, |rate| rate.charge(overage)));
                }
                Aggregation::AverageOfSamples => {
                    let overage = if billed {
                        used.saturating_sub(included)
                    } else {
                        0
                    };
                    quantity_weighted += u128::from(used) * u128::from(segment.milliseconds);
                    included_weighted += u128::from(included) * u128::from(segment.milliseconds);
                    overage_weighted += u128::from(overage) * u128::from(segment.milliseconds);
                    amount = amount.saturating_add(prorate_amount(
                        rate.map_or(0, |rate| rate.charge(overage)),
                        segment.milliseconds,
                        period_milliseconds,
                    ));
                }
            }
        }
        let (quantity, overage) = match aggregation(resource) {
            Aggregation::SumOfRecords => (quantity_summed, overage_flow),
            Aggregation::AverageOfSamples => (
                weighted_average(quantity_weighted, period_milliseconds),
                weighted_average(overage_weighted, period_milliseconds),
            ),
        };
        total = total.saturating_add(amount);
        line_items.push(LineItem {
            resource,
            quantity,
            included: weighted_average(included_weighted, period_milliseconds),
            overage,
            amount_micro_dollars: amount,
        });
    }

    Some(RatedPeriod {
        plan_id: current.plan.id.clone(),
        plan_version: current.plan.version,
        rate_card_version: card.version,
        base_micro_dollars: base,
        line_items,
        total_micro_dollars: total,
    })
}

/// A non-negative amount's share of the period, rounded down.
fn prorate_amount(
    amount: MicroDollars,
    milliseconds: u64,
    period_milliseconds: u64,
) -> MicroDollars {
    if period_milliseconds == 0 {
        return 0;
    }
    let share =
        i128::from(amount.max(0)) * i128::from(milliseconds) / i128::from(period_milliseconds);
    i64::try_from(share).unwrap_or(i64::MAX)
}

/// A quantity's share of the period, rounded down.
fn prorate_quantity(quantity: u64, milliseconds: u64, period_milliseconds: u64) -> u64 {
    if period_milliseconds == 0 {
        return 0;
    }
    let share = u128::from(quantity) * u128::from(milliseconds) / u128::from(period_milliseconds);
    u64::try_from(share).unwrap_or(u64::MAX)
}

fn weighted_average(weighted: u128, period_milliseconds: u64) -> u64 {
    if period_milliseconds == 0 {
        return 0;
    }
    u64::try_from(weighted / u128::from(period_milliseconds)).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn pro() -> Plan {
        plan("pro").expect("pro plan")
    }

    #[test]
    fn streaming_quantities_preserve_overflow_and_sample_rounding() {
        let mut quantity = PeriodQuantity::default();
        assert_eq!(quantity.quantity(QuotaResource::StorageBytes), 0);
        quantity.record(u64::MAX);
        quantity.record(u64::MAX);
        quantity.record(0);
        assert_eq!(
            quantity.quantity(QuotaResource::ObjectEgressBytesPerMonth),
            u64::MAX
        );
        assert_eq!(
            quantity.quantity(QuotaResource::StorageBytes),
            ((u128::from(u64::MAX) * 2) / 3) as u64
        );
    }

    #[test]
    fn project_costs_split_usage_without_repeating_the_base_fee() {
        let rated = rate_period(
            &pro(),
            &default_rate_card(),
            &BTreeMap::from([(QuotaResource::StorageBytes, 16 * GIB)]),
        );
        let quantities = BTreeMap::from([
            (
                "a".to_owned(),
                BTreeMap::from([(QuotaResource::StorageBytes, 4 * GIB)]),
            ),
            (
                "b".to_owned(),
                BTreeMap::from([(QuotaResource::StorageBytes, 12 * GIB)]),
            ),
            ("empty".to_owned(), BTreeMap::new()),
        ]);
        let costs = allocate_project_costs(&rated, &quantities);
        let sum = |id: &str| {
            costs[id]
                .iter()
                .map(|line| line.amount_micro_dollars)
                .sum::<i64>()
        };
        assert_eq!(sum("a"), 250_000);
        assert_eq!(sum("b"), 750_000);
        assert_eq!(sum("empty"), 0);
        assert_eq!(
            sum("a") + sum("b") + rated.base_micro_dollars,
            rated.total_micro_dollars
        );
    }

    #[test]
    fn project_allocation_preserves_rounding_and_handles_large_quantities() {
        for amount in [1, 2, 19, i64::MAX] {
            let rated = RatedPeriod {
                plan_id: "pro".to_owned(),
                plan_version: 1,
                rate_card_version: 1,
                base_micro_dollars: 0,
                total_micro_dollars: amount,
                line_items: vec![LineItem {
                    resource: QuotaResource::StorageBytes,
                    quantity: u64::MAX,
                    included: 0,
                    overage: u64::MAX,
                    amount_micro_dollars: amount,
                }],
            };
            let quantities = BTreeMap::from([
                (
                    "a".to_owned(),
                    BTreeMap::from([(QuotaResource::StorageBytes, u64::MAX)]),
                ),
                (
                    "b".to_owned(),
                    BTreeMap::from([(QuotaResource::StorageBytes, u64::MAX)]),
                ),
                ("zero".to_owned(), BTreeMap::new()),
            ]);
            let costs = allocate_project_costs(&rated, &quantities);
            assert_eq!(costs["a"][0].amount_micro_dollars, amount / 2);
            assert_eq!(costs["b"][0].amount_micro_dollars, amount - amount / 2);
            assert_eq!(costs["zero"][0].amount_micro_dollars, 0);
        }
    }

    #[test]
    fn free_and_unused_projects_have_no_allocated_costs() {
        for plan_id in ["free", "pro"] {
            let rated = rate_period(
                &plan(plan_id).expect("plan"),
                &default_rate_card(),
                &BTreeMap::new(),
            );
            let costs = allocate_project_costs(
                &rated,
                &BTreeMap::from([("empty".to_owned(), BTreeMap::new())]),
            );
            assert!(
                costs["empty"]
                    .iter()
                    .all(|line| line.quantity == 0 && line.amount_micro_dollars == 0)
            );
        }
    }

    #[test]
    fn a_free_organization_is_billed_nothing() {
        let free = plan("free").expect("free plan");
        let usage = BTreeMap::from([
            (QuotaResource::StorageBytes, 400 * 1024 * 1024),
            (QuotaResource::EdgeInvocationsPerMonth, 400_000),
        ]);
        let rated = rate_period(&free, &default_rate_card(), &usage);
        assert_eq!(rated.total_micro_dollars, 0);
        assert!(
            rated
                .line_items
                .iter()
                .all(|item| item.amount_micro_dollars == 0)
        );
    }

    #[test]
    fn a_pro_organization_inside_its_plan_pays_the_base_and_nothing_else() {
        let usage = BTreeMap::from([(QuotaResource::StorageBytes, 4 * GIB)]);
        let rated = rate_period(&pro(), &default_rate_card(), &usage);
        assert_eq!(rated.base_micro_dollars, 25_000_000);
        assert_eq!(rated.total_micro_dollars, 25_000_000);
    }

    #[test]
    fn overage_is_charged_per_priced_unit_with_the_partial_unit_free() {
        // 8 GiB included; 10.5 GiB stored -> 2.5 GiB over -> 2 whole priced
        // units. The half gigabyte is the tenant's, not a rounded-up charge
        // for capacity they did not use.
        let usage = BTreeMap::from([(QuotaResource::StorageBytes, 10 * GIB + GIB / 2)]);
        let rated = rate_period(&pro(), &default_rate_card(), &usage);
        let storage = rated
            .line_items
            .iter()
            .find(|item| item.resource == QuotaResource::StorageBytes)
            .expect("storage line");
        assert_eq!(storage.overage, 2 * GIB + GIB / 2);
        assert_eq!(storage.amount_micro_dollars, 2 * 125_000);
        assert_eq!(rated.total_micro_dollars, 25_000_000 + 250_000);
    }

    #[test]
    fn samples_of_a_level_average_instead_of_multiplying_the_charge() {
        // Twelve samples of the same nine gigabytes are one stored gigabyte of
        // overage, not twelve. Summing samples would bill the tenant once per
        // measurement for data they stored once -- the single easiest way for
        // this module to overcharge.
        let samples: Vec<u64> = vec![9 * GIB; 12];
        assert_eq!(
            period_quantity(QuotaResource::StorageBytes, &samples),
            9 * GIB
        );
        // Whereas a flow is genuinely the sum of its records.
        let transfers: Vec<u64> = vec![GIB; 12];
        assert_eq!(
            period_quantity(QuotaResource::ReplicationBytesPerMonth, &transfers),
            12 * GIB
        );
    }

    #[test]
    fn a_shrinking_tenant_pays_for_the_average_not_the_peak() {
        // Half a period at 16 GiB and half at zero is eight on average: the
        // mid-period delete halves the bill rather than being ignored.
        let samples: Vec<u64> = [vec![16 * GIB; 6], vec![0; 6]].concat();
        assert_eq!(
            period_quantity(QuotaResource::StorageBytes, &samples),
            8 * GIB
        );
    }

    /// Stored objects and their downloads are two more line items on the
    /// bill, one a level and one a flow. On the free plan both cap: use past
    /// the allowance is refused upstream, so the bill shows the excess as
    /// overage of zero and charges nothing for it.
    #[test]
    fn a_free_plan_lists_object_storage_and_egress_but_caps_instead_of_billing() {
        let free = plan("free").expect("free plan");
        // Three samples of the same two stored gigabytes are two gigabytes,
        // not six; two downloads of three gigabytes are six served.
        let stored = period_quantity(QuotaResource::ObjectStorageBytes, &[2 * GIB; 3]);
        let served = period_quantity(QuotaResource::ObjectEgressBytesPerMonth, &[3 * GIB; 2]);
        assert_eq!(stored, 2 * GIB);
        assert_eq!(served, 6 * GIB);
        let usage = BTreeMap::from([
            (QuotaResource::ObjectStorageBytes, stored),
            (QuotaResource::ObjectEgressBytesPerMonth, served),
        ]);

        let rated = rate_period(&free, &default_rate_card(), &usage);
        let line = |resource| {
            rated
                .line_items
                .iter()
                .find(|item| item.resource == resource)
                .cloned()
                .expect("line item")
        };
        assert_eq!(
            line(QuotaResource::ObjectStorageBytes),
            LineItem {
                resource: QuotaResource::ObjectStorageBytes,
                quantity: 2 * GIB,
                included: GIB,
                overage: 0,
                amount_micro_dollars: 0,
            }
        );
        assert_eq!(
            line(QuotaResource::ObjectEgressBytesPerMonth),
            LineItem {
                resource: QuotaResource::ObjectEgressBytesPerMonth,
                quantity: 6 * GIB,
                included: 5 * GIB,
                overage: 0,
                amount_micro_dollars: 0,
            }
        );
        assert_eq!(rated.total_micro_dollars, 0, "free stayed free");
    }

    /// On the pro plan the same two resources bill their excess: stored
    /// object bytes at $0.02 per GiB-month over 50 GiB, downloads at $0.09
    /// per GiB over 250 GiB, each rounded down to whole priced units.
    #[test]
    fn a_pro_plan_bills_object_storage_per_gib_month_and_egress_per_gib() {
        // 60 GiB for half the samples and 44 GiB for the other half average
        // to 52 GiB: two gigabytes over the included fifty.
        let stored = period_quantity(QuotaResource::ObjectStorageBytes, &[60 * GIB, 44 * GIB]);
        // 260.5 GiB served: ten and a half over, so ten priced units.
        let served = period_quantity(
            QuotaResource::ObjectEgressBytesPerMonth,
            &[200 * GIB, 60 * GIB + GIB / 2],
        );
        let usage = BTreeMap::from([
            (QuotaResource::ObjectStorageBytes, stored),
            (QuotaResource::ObjectEgressBytesPerMonth, served),
        ]);

        let rated = rate_period(&pro(), &default_rate_card(), &usage);
        let line = |resource| {
            rated
                .line_items
                .iter()
                .find(|item| item.resource == resource)
                .cloned()
                .expect("line item")
        };
        assert_eq!(
            line(QuotaResource::ObjectStorageBytes),
            LineItem {
                resource: QuotaResource::ObjectStorageBytes,
                quantity: 52 * GIB,
                included: 50 * GIB,
                overage: 2 * GIB,
                amount_micro_dollars: 2 * 20_000,
            }
        );
        assert_eq!(
            line(QuotaResource::ObjectEgressBytesPerMonth),
            LineItem {
                resource: QuotaResource::ObjectEgressBytesPerMonth,
                quantity: 260 * GIB + GIB / 2,
                included: 250 * GIB,
                overage: 10 * GIB + GIB / 2,
                amount_micro_dollars: 10 * 90_000,
            }
        );
        assert_eq!(
            rated.total_micro_dollars,
            25_000_000 + 2 * 20_000 + 10 * 90_000
        );
    }

    #[test]
    fn a_rated_period_re_derives_to_the_same_bill() {
        let usage = BTreeMap::from([
            (QuotaResource::StorageBytes, 20 * GIB),
            (QuotaResource::ApplicationUsers, 150_000),
            (QuotaResource::EdgeInvocationsPerMonth, 3_500_000),
        ]);
        let first = rate_period(&pro(), &default_rate_card(), &usage);
        let again = rate_period(&pro(), &default_rate_card(), &usage);
        assert_eq!(first, again, "rating the same period twice disagreed");

        // And it survives being stored and read back, which is what retention
        // for the dispute window depends on.
        let stored = serde_json::to_vec(&first).expect("serialize");
        let reread: RatedPeriod = serde_json::from_slice(&stored).expect("deserialize");
        assert_eq!(reread, first);
    }

    #[test]
    fn a_rate_card_of_zeroes_is_valid_and_bills_zero() {
        // The pipeline has to be able to run before any price is chosen.
        let zeroes = RateCard {
            version: 1,
            base_micro_dollars: BTreeMap::new(),
            overage: BTreeMap::new(),
        };
        let usage = BTreeMap::from([(QuotaResource::StorageBytes, 100 * GIB)]);
        let rated = rate_period(&pro(), &zeroes, &usage);
        assert_eq!(rated.total_micro_dollars, 0);
        // The overage is still visible on the line item even though it costs
        // nothing, so the bill explains itself either way.
        assert!(
            rated
                .line_items
                .iter()
                .any(|item| item.resource == QuotaResource::StorageBytes && item.overage > 0)
        );
    }

    /// Proration must vanish when there is nothing to prorate: one plan for
    /// the whole period has to rate to the identical bill, line for line, or
    /// every unchanged organization's bill moved the day proration landed.
    #[test]
    fn one_segment_covering_the_period_rates_exactly_like_the_unsegmented_period() {
        let usage = BTreeMap::from([
            (QuotaResource::StorageBytes, 10 * GIB + GIB / 2),
            (QuotaResource::EdgeInvocationsPerMonth, 3_500_000),
        ]);
        let whole = rate_period(&pro(), &default_rate_card(), &usage);
        let segmented = rate_period_prorated(
            &default_rate_card(),
            &[PlanSegment {
                plan: pro(),
                milliseconds: 30 * 24 * 60 * 60 * 1_000,
                usage,
            }],
        )
        .expect("rated");
        assert_eq!(segmented, whole);
    }

    /// An upgrade partway through the month must charge each plan for its
    /// share: the base fee prorates by time, a flow's allowance prorates the
    /// same way, and use under the free stretch stays uncharged because a
    /// plan that cannot bill overage cannot start billing it retroactively.
    #[test]
    fn an_upgrade_mid_month_prorates_the_base_and_the_flow_allowances() {
        const MONTH: u64 = 30 * 24 * 60 * 60 * 1_000;
        let free = plan("free").expect("free plan");
        let free_invocations = free
            .entitlement(QuotaResource::EdgeInvocationsPerMonth)
            .expect("free entitlement");
        assert!(!free_invocations.overage_billed);
        let pro_included = pro()
            .entitlement(QuotaResource::EdgeInvocationsPerMonth)
            .expect("pro entitlement")
            .included;

        // Ten days free, twenty days pro. The pro stretch served included
        // invocations plus exactly three million beyond its share.
        let pro_use = pro_included * 2 / 3 + 3_000_000;
        let rated = rate_period_prorated(
            &default_rate_card(),
            &[
                PlanSegment {
                    plan: free,
                    milliseconds: MONTH / 3,
                    usage: BTreeMap::from([(QuotaResource::EdgeInvocationsPerMonth, 100_000)]),
                },
                PlanSegment {
                    plan: pro(),
                    milliseconds: MONTH * 2 / 3,
                    usage: BTreeMap::from([(QuotaResource::EdgeInvocationsPerMonth, pro_use)]),
                },
            ],
        )
        .expect("rated");

        // Two thirds of the month on pro is two thirds of the base fee.
        assert_eq!(rated.base_micro_dollars, 25_000_000 * 2 / 3);
        assert_eq!(rated.plan_id, "pro", "the bill names the current plan");
        let invocations = rated
            .line_items
            .iter()
            .find(|item| item.resource == QuotaResource::EdgeInvocationsPerMonth)
            .expect("invocations line");
        // The free stretch's use is uncharged; the pro stretch owes for three
        // million beyond its prorated allowance, at $2 per million.
        assert_eq!(invocations.overage, 3_000_000);
        assert_eq!(invocations.amount_micro_dollars, 3 * 2_000_000);
        assert_eq!(
            rated.total_micro_dollars,
            rated.base_micro_dollars + 3 * 2_000_000
        );
    }

    /// A level is a height, so a changed plan splits its charge by time held,
    /// not by comparing against a shrunken allowance: nine gigabytes above
    /// the included level for half the pro stretch is half that stretch's
    /// charge.
    #[test]
    fn a_level_prorates_its_charge_by_the_time_it_was_held() {
        const MONTH: u64 = 30 * 24 * 60 * 60 * 1_000;
        // Pro for the whole month, but measured as two equal stretches to
        // prove the split arithmetic: 16 GiB stored in one half, 8 GiB (the
        // included level) in the other.
        let rated = rate_period_prorated(
            &default_rate_card(),
            &[
                PlanSegment {
                    plan: pro(),
                    milliseconds: MONTH / 2,
                    usage: BTreeMap::from([(QuotaResource::StorageBytes, 16 * GIB)]),
                },
                PlanSegment {
                    plan: pro(),
                    milliseconds: MONTH / 2,
                    usage: BTreeMap::from([(QuotaResource::StorageBytes, 8 * GIB)]),
                },
            ],
        )
        .expect("rated");
        let storage = rated
            .line_items
            .iter()
            .find(|item| item.resource == QuotaResource::StorageBytes)
            .expect("storage line");
        // Eight gigabytes over included, held for half the period: half of
        // eight priced units.
        assert_eq!(storage.amount_micro_dollars, 8 * 125_000 / 2);
        assert_eq!(
            storage.quantity,
            12 * GIB,
            "the shown level is the time-weighted average"
        );
        // No time at all is not a period; rating refuses to invent one.
        assert!(rate_period_prorated(&default_rate_card(), &[]).is_none());
    }
}
