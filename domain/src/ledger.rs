//! Returnables ledger: balances roll cages delivered against empties
//! collected on store visits.
//!
//! Each delivery hands the store `Store::demand` handling units; each visit
//! collects the banner's configured empties
//! ([`crate::model::ChainRules::returnables_for`]) back onto the truck. The outstanding
//! balance is what must still be recovered from that store:
//!
//! ```text
//! outstanding = issued - collected   (per compartment, floored at zero)
//! ```
//!
//! The collection figures are illustrative fixture data, not researched
//! operator data. A plan that never visits a store has no ledger row for it:
//! nothing was issued and nothing collected.

use crate::ids::StoreId;
use crate::model::{Load, Node, Problem, Solution};

/// One store's returnables balance over a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreBalance {
    pub store: StoreId,
    /// Times the plan visits the store.
    pub visits: u32,
    /// Cages delivered (demand summed over visits).
    pub issued: Load,
    /// Empties collected (banner fixture per visit, summed).
    pub collected: Load,
    /// `issued - collected` per compartment, floored at zero.
    pub outstanding: Load,
}

/// Ledger over every store the plan serves, plus fleet-wide totals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReturnablesLedger {
    pub stores: Vec<StoreBalance>,
    pub total_issued: Load,
    pub total_collected: Load,
    pub total_outstanding: Load,
}

/// Builds the returnables ledger for `solution` against `problem`.
///
/// Stores listed in `Solution::unassigned` produce no rows.
#[must_use]
pub fn ledger(problem: &Problem, solution: &Solution) -> ReturnablesLedger {
    let mut visits: Vec<u32> = vec![0; problem.stores.len()];
    for route in &solution.routes {
        for step in &route.steps {
            if let Node::Store(id) = step.node
                && let Some(count) = visits.get_mut(id.0 as usize)
            {
                *count = count.saturating_add(1);
            }
        }
    }

    let mut balances = Vec::new();
    for store in &problem.stores {
        let count = visits.get(store.id.0 as usize).copied().unwrap_or(0);
        if count == 0 {
            continue;
        }
        let per_visit = problem.chain_rules.returnables_for(store.chain);
        let issued = scale(store.demand, count);
        let collected = scale(per_visit, count);
        balances.push(StoreBalance {
            store: store.id,
            visits: count,
            issued,
            collected,
            outstanding: difference(issued, collected),
        });
    }

    let total_issued = balances
        .iter()
        .fold(Load::ZERO, |total, balance| add(total, balance.issued));
    let total_collected = balances
        .iter()
        .fold(Load::ZERO, |total, balance| add(total, balance.collected));
    let total_outstanding = balances
        .iter()
        .fold(Load::ZERO, |total, balance| add(total, balance.outstanding));

    ReturnablesLedger {
        stores: balances,
        total_issued,
        total_collected,
        total_outstanding,
    }
}

/// Multiplies a load by an integer count, saturating.
fn scale(load: Load, count: u32) -> Load {
    Load::new(
        load.ambient.saturating_mul(count),
        load.chilled.saturating_mul(count),
        load.frozen.saturating_mul(count),
    )
}

/// Componentwise `issued - collected`, floored at zero.
fn difference(issued: Load, collected: Load) -> Load {
    Load::new(
        issued.ambient.saturating_sub(collected.ambient),
        issued.chilled.saturating_sub(collected.chilled),
        issued.frozen.saturating_sub(collected.frozen),
    )
}

/// Componentwise sum, saturating.
fn add(a: Load, b: Load) -> Load {
    Load::new(
        a.ambient.saturating_add(b.ambient),
        a.chilled.saturating_add(b.chilled),
        a.frozen.saturating_add(b.frozen),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::LatLon;
    use crate::ids::{DepotId, DriverId, VehicleId};
    use crate::model::{
        Chain, ChainRules, CostModel, Depot, Driver, FatigueRules, PlannedStop, Problem,
        ReturnablesDefaults, RoutePlan, Shift, Store, TimeWindow, Vehicle,
    };
    use crate::time::Minutes;

    fn problem_with(demand: Load, collected: Load) -> Problem {
        Problem {
            name: "ledger fixture".into(),
            depots: vec![Depot {
                id: DepotId(0),
                name: "DC".into(),
                location: LatLon::new(-33.87, 151.21),
                address: None,
                timezone: None,
            }],
            stores: vec![
                Store {
                    id: StoreId(0),
                    chain: Chain::Coles,
                    name: "A".into(),
                    location: LatLon::new(-33.80, 151.30),
                    address: None,
                    demand,
                    service: Minutes::from_minutes(10),
                    receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
                    timezone: None,
                },
                Store {
                    id: StoreId(1),
                    chain: Chain::Aldi,
                    name: "B".into(),
                    location: LatLon::new(-33.90, 151.10),
                    address: None,
                    demand: Load::new(9, 9, 9),
                    service: Minutes::from_minutes(10),
                    receiving: vec![TimeWindow::new(Minutes::ZERO, Minutes::from_minutes(600))],
                    timezone: None,
                },
            ],
            vehicles: vec![Vehicle {
                id: VehicleId(0),
                name: "Rigid".into(),
                depot: DepotId(0),
                capacity: Load::new(99, 99, 99),
                max_duration: Minutes::from_minutes(600),
                avg_speed_kmh: 60.0,
            }],
            drivers: vec![Driver {
                id: DriverId(0),
                name: "Sam".into(),
                shifts: vec![Shift::new(Minutes::ZERO, Minutes::from_minutes(600))],
                fatigue_rules: FatigueRules::default(),
            }],
            cost: CostModel::default(),
            chain_rules: ChainRules {
                returnables: ReturnablesDefaults {
                    coles: collected,
                    ..ReturnablesDefaults::default()
                },
                ..ChainRules::default()
            },
            curfews: Default::default(),
            horizon_start: None,
        }
    }

    fn solution_visiting(stores: &[StoreId]) -> Solution {
        let mut steps = vec![PlannedStop {
            node: Node::Depot(DepotId(0)),
            arrive: Minutes::ZERO,
            depart: Minutes::ZERO,
        }];
        for &store in stores {
            steps.push(PlannedStop {
                node: Node::Store(store),
                arrive: Minutes::from_minutes(30),
                depart: Minutes::from_minutes(40),
            });
        }
        steps.push(PlannedStop {
            node: Node::Depot(DepotId(0)),
            arrive: Minutes::from_minutes(70),
            depart: Minutes::from_minutes(70),
        });
        Solution {
            routes: vec![RoutePlan {
                vehicle: VehicleId(0),
                drivers: vec![DriverId(0)],
                steps,
                distance_km: 10.0,
                duration: Minutes::from_minutes(70),
                cost_cents: 0,
            }],
            unassigned: Vec::new(),
            total_distance_km: 10.0,
            total_duration: Minutes::from_minutes(70),
            total_cost_cents: 0,
        }
    }

    #[test]
    fn balances_issued_against_collected() {
        let problem = problem_with(Load::new(6, 4, 2), Load::new(2, 2, 1));
        let solution = solution_visiting(&[StoreId(0)]);
        let ledger = ledger(&problem, &solution);

        assert_eq!(ledger.stores.len(), 1);
        let balance = ledger.stores[0];
        assert_eq!(balance.store, StoreId(0));
        assert_eq!(balance.visits, 1);
        assert_eq!(balance.issued, Load::new(6, 4, 2));
        assert_eq!(balance.collected, Load::new(2, 2, 1));
        assert_eq!(balance.outstanding, Load::new(4, 2, 1));
        assert_eq!(ledger.total_issued, Load::new(6, 4, 2));
        assert_eq!(ledger.total_collected, Load::new(2, 2, 1));
        assert_eq!(ledger.total_outstanding, Load::new(4, 2, 1));
    }

    #[test]
    fn collection_is_floored_at_zero_per_compartment() {
        let problem = problem_with(Load::new(1, 1, 5), Load::new(4, 0, 5));
        let solution = solution_visiting(&[StoreId(0)]);
        let ledger = ledger(&problem, &solution);

        // Collected more ambient than issued (empties from earlier trips):
        // outstanding floors at zero rather than going negative.
        assert_eq!(ledger.stores[0].outstanding, Load::new(0, 1, 0));
    }

    #[test]
    fn unserved_stores_have_no_rows_and_no_zero_rule_collects_nothing() {
        let problem = problem_with(Load::new(6, 4, 2), Load::ZERO);
        let solution = solution_visiting(&[StoreId(1)]);
        let ledger = ledger(&problem, &solution);

        assert_eq!(ledger.stores.len(), 1);
        assert_eq!(ledger.stores[0].store, StoreId(1));
        assert_eq!(ledger.stores[0].collected, Load::ZERO);
        assert_eq!(ledger.stores[0].outstanding, Load::new(9, 9, 9));
    }

    #[test]
    fn totals_sum_across_stores() {
        let problem = problem_with(Load::new(6, 4, 2), Load::new(1, 1, 1));
        let solution = solution_visiting(&[StoreId(0), StoreId(1)]);
        let ledger = ledger(&problem, &solution);

        assert_eq!(ledger.stores.len(), 2);
        // Store 1 is ALDI: the fixture only sets Coles returns.
        assert_eq!(ledger.stores[1].collected, Load::ZERO);
        assert_eq!(ledger.total_issued, Load::new(15, 13, 11));
        assert_eq!(ledger.total_collected, Load::new(1, 1, 1));
        assert_eq!(ledger.total_outstanding, Load::new(14, 12, 10));
    }
}
