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
        | QuotaResource::ApplicationUsers
        | QuotaResource::Environments
        | QuotaResource::CollectionsPerEnvironment
        | QuotaResource::EdgeFunctions => Aggregation::AverageOfSamples,
        QuotaResource::ReplicationRequestsPerMinute
        | QuotaResource::ReplicationBytesPerMonth
        | QuotaResource::EdgeInvocationsPerMonth
        | QuotaResource::EdgeComputeMillisecondsPerMonth
        | QuotaResource::LogBytesPerMonth => Aggregation::SumOfRecords,
    }
}

/// Reduce a period's usage records for one resource to the quantity billed.
#[must_use]
pub fn period_quantity(resource: QuotaResource, records: &[u64]) -> u64 {
    if records.is_empty() {
        return 0;
    }
    match aggregation(resource) {
        Aggregation::SumOfRecords => records
            .iter()
            .fold(0_u64, |total, record| total.saturating_add(*record)),
        Aggregation::AverageOfSamples => {
            let total: u128 = records.iter().map(|record| u128::from(*record)).sum();
            u64::try_from(total / records.len() as u128).unwrap_or(u64::MAX)
        }
    }
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
/// monthly active user beyond 100,000, and $2 per million invocations beyond
/// two million. Until the beta ends nothing is collected regardless.
#[must_use]
pub fn default_rate_card() -> RateCard {
    const GIB: u64 = 1024 * 1024 * 1024;
    RateCard {
        version: 1,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn pro() -> Plan {
        plan("pro").expect("pro plan")
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
}
