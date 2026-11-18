//! Truck Load & Capacity Optimizer UI and packing engine for the **Truck Load** tab.
//!
//! Computes physical floor layout (standard, 90°-rotated, and mixed-row packing),
//! vertical stacking, volume/cube utilization, payload weight limits, axle/CoG
//! balance, and LIFO stop-by-stop slot assignments, and renders an interactive
//! three-view `egui::Painter` display (top-down cargo deck, side elevation, and
//! rear cross-section).

use std::collections::BTreeMap;

use domain::{Load, Node, Problem, RoutePlan};
use eframe::egui;

use crate::app::{SolutionBundle, stat_card, vehicle_name};

const EPS: f64 = 1e-9;

/// Common Australian/commercial delivery vehicle body presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TruckPreset {
    Custom,
    DeliveryVan35t,
    LightRigid8t,
    #[default]
    MediumRigid14t,
    HeavyRigid225t,
    SemiTrailer425t,
    BDouble625t,
}

impl TruckPreset {
    pub(crate) const ALL: [Self; 7] = [
        Self::DeliveryVan35t,
        Self::LightRigid8t,
        Self::MediumRigid14t,
        Self::HeavyRigid225t,
        Self::SemiTrailer425t,
        Self::BDouble625t,
        Self::Custom,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Custom => "Custom Dimensions",
            Self::DeliveryVan35t => "Delivery Van (3.5t GVM)",
            Self::LightRigid8t => "Light Rigid (8.0t GVM)",
            Self::MediumRigid14t => "Medium Rigid (14.0t GVM)",
            Self::HeavyRigid225t => "Heavy Rigid 3-Axle (22.5t GVM)",
            Self::SemiTrailer425t => "Semi-Trailer (42.5t GCM)",
            Self::BDouble625t => "B-Double Combination (62.5t GCM)",
        }
    }

    pub(crate) fn dimensions(self) -> TruckDimensions {
        match self {
            Self::DeliveryVan35t => TruckDimensions {
                bay_length_m: 3.70,
                bay_width_m: 1.78,
                bay_height_m: 1.85,
                door_clearance_m: 1.72,
                bulkhead_thickness_m: 0.06,
                use_volume_override: false,
                volume_override_m3: 3.70 * 1.78 * 1.85,
                usable_volume_efficiency_pct: 100.0,
                max_payload_kg: 1_350.0,
                tare_weight_kg: 2_150.0,
                external_height_m: 2.65,
            },
            Self::LightRigid8t => TruckDimensions {
                bay_length_m: 4.80,
                bay_width_m: 2.20,
                bay_height_m: 2.10,
                door_clearance_m: 2.00,
                bulkhead_thickness_m: 0.08,
                use_volume_override: false,
                volume_override_m3: 4.80 * 2.20 * 2.10,
                usable_volume_efficiency_pct: 100.0,
                max_payload_kg: 4_200.0,
                tare_weight_kg: 3_800.0,
                external_height_m: 3.20,
            },
            Self::MediumRigid14t | Self::Custom => TruckDimensions {
                bay_length_m: 6.60,
                bay_width_m: 2.40,
                bay_height_m: 2.30,
                door_clearance_m: 2.20,
                bulkhead_thickness_m: 0.08,
                use_volume_override: false,
                volume_override_m3: 6.60 * 2.40 * 2.30,
                usable_volume_efficiency_pct: 100.0,
                max_payload_kg: 7_800.0,
                tare_weight_kg: 6_200.0,
                external_height_m: 3.60,
            },
            Self::HeavyRigid225t => TruckDimensions {
                bay_length_m: 8.80,
                bay_width_m: 2.44,
                bay_height_m: 2.45,
                door_clearance_m: 2.35,
                bulkhead_thickness_m: 0.09,
                use_volume_override: false,
                volume_override_m3: 8.80 * 2.44 * 2.45,
                usable_volume_efficiency_pct: 100.0,
                max_payload_kg: 13_000.0,
                tare_weight_kg: 9_500.0,
                external_height_m: 3.90,
            },
            Self::SemiTrailer425t => TruckDimensions {
                bay_length_m: 13.60,
                bay_width_m: 2.45,
                bay_height_m: 2.65,
                door_clearance_m: 2.55,
                bulkhead_thickness_m: 0.10,
                use_volume_override: false,
                volume_override_m3: 13.60 * 2.45 * 2.65,
                usable_volume_efficiency_pct: 100.0,
                max_payload_kg: 24_500.0,
                tare_weight_kg: 18_000.0,
                external_height_m: 4.30,
            },
            Self::BDouble625t => TruckDimensions {
                bay_length_m: 19.20,
                bay_width_m: 2.45,
                bay_height_m: 2.65,
                door_clearance_m: 2.55,
                bulkhead_thickness_m: 0.10,
                use_volume_override: false,
                volume_override_m3: 19.20 * 2.45 * 2.65,
                usable_volume_efficiency_pct: 100.0,
                max_payload_kg: 38_000.0,
                tare_weight_kg: 24_500.0,
                external_height_m: 4.30,
            },
        }
    }
}

/// Standard handling-unit presets (roll cages, pallets, crates).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum UnitPreset {
    #[default]
    AuRollCage,
    AuStandardPallet,
    EuroPallet,
    PlasticTote,
    Custom,
}

impl UnitPreset {
    pub(crate) const ALL: [Self; 5] = [
        Self::AuRollCage,
        Self::AuStandardPallet,
        Self::EuroPallet,
        Self::PlasticTote,
        Self::Custom,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AuRollCage => "AU Roll Cage (800×720×1700 mm)",
            Self::AuStandardPallet => "AU Standard Pallet (1165×1165×1400 mm)",
            Self::EuroPallet => "Euro / CHEP Pallet (1200×800×1400 mm)",
            Self::PlasticTote => "Delivery Tote / Crate (600×400×400 mm)",
            Self::Custom => "Custom Handling Unit",
        }
    }

    pub(crate) fn spec(self) -> HandlingUnitSpec {
        match self {
            Self::AuRollCage | Self::Custom => HandlingUnitSpec {
                length_m: 0.80,
                width_m: 0.72,
                height_m: 1.70,
                use_unit_volume_override: false,
                unit_volume_override_m3: 0.80 * 0.72 * 1.70,
                ambient_weight_kg: 220.0,
                chilled_weight_kg: 250.0,
                frozen_weight_kg: 260.0,
                max_stack_layers: 1,
                allow_rotation: true,
            },
            Self::AuStandardPallet => HandlingUnitSpec {
                length_m: 1.165,
                width_m: 1.165,
                height_m: 1.40,
                use_unit_volume_override: false,
                unit_volume_override_m3: 1.165 * 1.165 * 1.40,
                ambient_weight_kg: 550.0,
                chilled_weight_kg: 620.0,
                frozen_weight_kg: 650.0,
                max_stack_layers: 2,
                allow_rotation: false,
            },
            Self::EuroPallet => HandlingUnitSpec {
                length_m: 1.20,
                width_m: 0.80,
                height_m: 1.40,
                use_unit_volume_override: false,
                unit_volume_override_m3: 1.20 * 0.80 * 1.40,
                ambient_weight_kg: 480.0,
                chilled_weight_kg: 540.0,
                frozen_weight_kg: 560.0,
                max_stack_layers: 2,
                allow_rotation: true,
            },
            Self::PlasticTote => HandlingUnitSpec {
                length_m: 0.60,
                width_m: 0.40,
                height_m: 0.40,
                use_unit_volume_override: false,
                unit_volume_override_m3: 0.60 * 0.40 * 0.40,
                ambient_weight_kg: 18.0,
                chilled_weight_kg: 22.0,
                frozen_weight_kg: 24.0,
                max_stack_layers: 5,
                allow_rotation: true,
            },
        }
    }
}

/// Physical dimensions, volume rules, and weight ratings for the truck cargo bay.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TruckDimensions {
    pub(crate) bay_length_m: f64,
    pub(crate) bay_width_m: f64,
    pub(crate) bay_height_m: f64,
    pub(crate) door_clearance_m: f64,
    pub(crate) bulkhead_thickness_m: f64,
    pub(crate) use_volume_override: bool,
    pub(crate) volume_override_m3: f64,
    pub(crate) usable_volume_efficiency_pct: f64,
    pub(crate) max_payload_kg: f64,
    pub(crate) tare_weight_kg: f64,
    pub(crate) external_height_m: f64,
}

impl Default for TruckDimensions {
    fn default() -> Self {
        TruckPreset::MediumRigid14t.dimensions()
    }
}

impl TruckDimensions {
    pub(crate) fn gross_volume_m3(&self) -> f64 {
        (self.bay_length_m.max(0.0)) * (self.bay_width_m.max(0.0)) * (self.bay_height_m.max(0.0))
    }

    pub(crate) fn max_gvm_tonnes(&self) -> f64 {
        (self.tare_weight_kg.max(0.0) + self.max_payload_kg.max(0.0)) / 1000.0
    }
}

/// Physical dimensions and weight specs of a single cargo handling unit.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HandlingUnitSpec {
    pub(crate) length_m: f64,
    pub(crate) width_m: f64,
    pub(crate) height_m: f64,
    pub(crate) use_unit_volume_override: bool,
    pub(crate) unit_volume_override_m3: f64,
    pub(crate) ambient_weight_kg: f64,
    pub(crate) chilled_weight_kg: f64,
    pub(crate) frozen_weight_kg: f64,
    pub(crate) max_stack_layers: u32,
    pub(crate) allow_rotation: bool,
}

impl Default for HandlingUnitSpec {
    fn default() -> Self {
        UnitPreset::AuRollCage.spec()
    }
}

impl HandlingUnitSpec {
    pub(crate) fn effective_volume_m3(&self) -> f64 {
        if self.use_unit_volume_override {
            self.unit_volume_override_m3.max(0.0001)
        } else {
            (self.length_m.max(0.01)) * (self.width_m.max(0.01)) * (self.height_m.max(0.01))
        }
    }

    pub(crate) fn weight_for(&self, compartment: CompartmentKind) -> f64 {
        match compartment {
            CompartmentKind::Ambient => self.ambient_weight_kg.max(0.1),
            CompartmentKind::Chilled => self.chilled_weight_kg.max(0.1),
            CompartmentKind::Frozen => self.frozen_weight_kg.max(0.1),
        }
    }
}

/// Temperature-controlled cargo zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CompartmentKind {
    Frozen,
    Chilled,
    Ambient,
}

impl CompartmentKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Frozen => "Frozen",
            Self::Chilled => "Chilled",
            Self::Ambient => "Ambient",
        }
    }

    pub(crate) fn short_code(self) -> &'static str {
        match self {
            Self::Frozen => "F",
            Self::Chilled => "C",
            Self::Ambient => "A",
        }
    }

    pub(crate) fn color(self) -> egui::Color32 {
        match self {
            Self::Frozen => egui::Color32::from_rgb(76, 128, 224),
            Self::Chilled => egui::Color32::from_rgb(38, 166, 154),
            Self::Ambient => egui::Color32::from_rgb(230, 158, 48),
        }
    }
}

/// How total truck unit capacity should be split across temperature compartments.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum CompartmentSplitMode {
    #[default]
    MatchTargetLoad,
    ManualPercentages {
        ambient_pct: f64,
        chilled_pct: f64,
        frozen_pct: f64,
    },
    SingleZoneAmbient,
}

/// Which demand / route source fills the truck preview.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LoadTargetSource {
    #[default]
    DraftVehicle,
    DraftTotalDemand,
    SolvedRoute,
    CustomLoad,
}

impl LoadTargetSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::DraftVehicle => "Selected Draft Vehicle",
            Self::DraftTotalDemand => "All Draft Stores Combined",
            Self::SolvedRoute => "Solved Route (LIFO Stops)",
            Self::CustomLoad => "Custom Test Load",
        }
    }
}

/// Canvas visualization mode in the Truck Load tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TruckViewMode {
    #[default]
    AllViews,
    TopDownDeck,
    SideElevation,
    RearCrossSection,
}

impl TruckViewMode {
    pub(crate) const ALL: [Self; 4] = [
        Self::AllViews,
        Self::TopDownDeck,
        Self::SideElevation,
        Self::RearCrossSection,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AllViews => "Combined (3 Views)",
            Self::TopDownDeck => "Top-Down Deck",
            Self::SideElevation => "Side Elevation",
            Self::RearCrossSection => "Rear Cross-Section",
        }
    }
}

/// Persistent UI state for the **Truck Load** tab.
#[derive(Clone, Debug)]
pub(crate) struct TruckLoadState {
    pub(crate) truck_preset: TruckPreset,
    pub(crate) truck: TruckDimensions,
    pub(crate) unit_preset: UnitPreset,
    pub(crate) unit: HandlingUnitSpec,
    pub(crate) split_mode: CompartmentSplitMode,
    pub(crate) manual_ambient_pct: f64,
    pub(crate) manual_chilled_pct: f64,
    pub(crate) manual_frozen_pct: f64,
    pub(crate) target_source: LoadTargetSource,
    pub(crate) selected_vehicle_idx: usize,
    pub(crate) selected_route_idx: usize,
    pub(crate) custom_load: Load,
    pub(crate) preview_max_fill: bool,
    pub(crate) show_stop_labels: bool,
    pub(crate) view_mode: TruckViewMode,
    pub(crate) status_banner: Option<String>,
}

impl Default for TruckLoadState {
    fn default() -> Self {
        Self {
            truck_preset: TruckPreset::MediumRigid14t,
            truck: TruckPreset::MediumRigid14t.dimensions(),
            unit_preset: UnitPreset::AuRollCage,
            unit: UnitPreset::AuRollCage.spec(),
            split_mode: CompartmentSplitMode::MatchTargetLoad,
            manual_ambient_pct: 55.0,
            manual_chilled_pct: 30.0,
            manual_frozen_pct: 15.0,
            target_source: LoadTargetSource::DraftVehicle,
            selected_vehicle_idx: 0,
            selected_route_idx: 0,
            custom_load: Load::new(12, 6, 4),
            preview_max_fill: false,
            show_stop_labels: true,
            view_mode: TruckViewMode::AllViews,
            status_banner: None,
        }
    }
}

impl TruckLoadState {
    pub(crate) fn active_split_mode(&self) -> CompartmentSplitMode {
        match self.split_mode {
            CompartmentSplitMode::MatchTargetLoad => CompartmentSplitMode::MatchTargetLoad,
            CompartmentSplitMode::ManualPercentages { .. } => {
                CompartmentSplitMode::ManualPercentages {
                    ambient_pct: self.manual_ambient_pct,
                    chilled_pct: self.manual_chilled_pct,
                    frozen_pct: self.manual_frozen_pct,
                }
            }
            CompartmentSplitMode::SingleZoneAmbient => CompartmentSplitMode::SingleZoneAmbient,
        }
    }
}

/// Actions sent back from the Truck Load tab to `ConsoleApp`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TruckLoadAction {
    SetDraftVehicleCapacity {
        vehicle_idx: usize,
        capacity: Load,
    },
    SetAllDraftVehicleCapacities {
        capacity: Load,
    },
    SyncSolverLimits {
        max_weight_tonnes: f64,
        max_height_m: f64,
    },
}

/// Optimal floor orientation chosen by the packing solver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FloorOrientationChoice {
    Standard {
        rows: u32,
        cols: u32,
    },
    Rotated90 {
        rows: u32,
        cols: u32,
    },
    Mixed {
        std_rows: u32,
        std_cols: u32,
        rot_rows: u32,
        rot_cols: u32,
    },
}

impl FloorOrientationChoice {
    pub(crate) fn units_per_layer(self) -> u32 {
        match self {
            Self::Standard { rows, cols } | Self::Rotated90 { rows, cols } => rows * cols,
            Self::Mixed {
                std_rows,
                std_cols,
                rot_rows,
                rot_cols,
            } => std_rows * std_cols + rot_rows * rot_cols,
        }
    }

    pub(crate) fn label(self) -> String {
        match self {
            Self::Standard { rows, cols } => {
                format!("Lengthwise ({rows} rows × {cols} cols)")
            }
            Self::Rotated90 { rows, cols } => {
                format!("Rotated 90° ({rows} rows × {cols} cols)")
            }
            Self::Mixed {
                std_rows,
                std_cols,
                rot_rows,
                rot_cols,
            } => {
                format!("Hybrid Mixed ({std_rows}×{std_cols} std + {rot_rows}×{rot_cols} rot)")
            }
        }
    }
}

/// Primary physical constraint capping capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BottleneckKind {
    FloorFootprint,
    CeilingOrStackHeight,
    DoorClearance,
    UsableVolume,
    PayloadWeightLimit,
    Unconstrained,
}

impl BottleneckKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::FloorFootprint => "Floor Area / Dimensions",
            Self::CeilingOrStackHeight => "Bay Height / Stack Limit",
            Self::DoorClearance => "Rear Door Clearance",
            Self::UsableVolume => "Volume Capacity (m³)",
            Self::PayloadWeightLimit => "Payload Weight Limit (kg)",
            Self::Unconstrained => "Invalid / Zero Dimensions",
        }
    }
}

/// Longitudinal balance classification based on loaded center of gravity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AxleBalanceStatus {
    Unloaded,
    Balanced,
    FrontHeavy,
    RearHeavy,
}

impl AxleBalanceStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unloaded => "Empty",
            Self::Balanced => "Balanced Axle Split",
            Self::FrontHeavy => "Front-Heavy (Cab End)",
            Self::RearHeavy => "Rear-Heavy (Tailgate End)",
        }
    }
}

/// Delivery stop descriptor in route order (`sequence_no = 1` is first delivery).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StopLoadTag {
    pub(crate) sequence_no: usize,
    pub(crate) store_name: String,
    pub(crate) demand: Load,
}

/// Visual occupancy state for a floor slot on the truck deck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoadedSlotStatus {
    Loaded,
    EmptyCapacity,
    WeightOrVolumeLocked,
}

/// Single floor position rendered in the Top-Down and Side views.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FloorSlotPlan {
    pub(crate) row: u32,
    pub(crate) col: u32,
    pub(crate) x_start_m: f64,
    pub(crate) y_start_m: f64,
    pub(crate) slot_length_m: f64,
    pub(crate) slot_width_m: f64,
    pub(crate) rotated: bool,
    pub(crate) compartment: CompartmentKind,
    pub(crate) status: LoadedSlotStatus,
    pub(crate) loaded_layers: u32,
    pub(crate) max_layers: u32,
    pub(crate) slot_weight_kg: f64,
    pub(crate) stop_badges: Vec<String>,
    pub(crate) tooltip: String,
}

/// Longitudinal zone span along the cargo floor for rendering compartment headers.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ZoneSpan {
    pub(crate) compartment: CompartmentKind,
    pub(crate) x_start_m: f64,
    pub(crate) x_end_m: f64,
    pub(crate) capacity_units: u32,
    pub(crate) loaded_units: u32,
}

/// Resolved target load and optional delivery stop list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TargetResolution {
    pub(crate) source_label: String,
    pub(crate) target_load: Load,
    pub(crate) total_problem_demand: Load,
    pub(crate) stops: Vec<StopLoadTag>,
}

/// Full result of [`compute_truck_capacity_plan`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TruckCapacityPlan {
    pub(crate) target_label: String,
    pub(crate) active_zones: usize,
    pub(crate) active_bulkheads: usize,
    pub(crate) usable_length_m: f64,
    pub(crate) effective_ceiling_m: f64,
    pub(crate) gross_bay_volume_m3: f64,
    pub(crate) usable_bay_volume_m3: f64,
    pub(crate) unit_volume_m3: f64,
    pub(crate) orientation: FloorOrientationChoice,
    pub(crate) rotation_gain_units: u32,
    pub(crate) units_per_layer: u32,
    pub(crate) stack_layers: u32,
    pub(crate) geometric_max_units: u32,
    pub(crate) volume_max_units: u32,
    pub(crate) weight_max_units: u32,
    pub(crate) effective_max_units: u32,
    pub(crate) bottleneck: BottleneckKind,
    pub(crate) recommended_capacity: Load,
    pub(crate) displayed_load: Load,
    pub(crate) target_load: Load,
    pub(crate) overflow_load: Load,
    pub(crate) cube_utilization_pct: f64,
    pub(crate) usable_cube_utilization_pct: f64,
    pub(crate) floor_utilization_pct: f64,
    pub(crate) weight_utilization_pct: f64,
    pub(crate) loaded_volume_m3: f64,
    pub(crate) dead_volume_m3: f64,
    pub(crate) loaded_payload_kg: f64,
    pub(crate) total_gvm_tonnes: f64,
    pub(crate) cog_from_front_pct: f64,
    pub(crate) axle_balance: AxleBalanceStatus,
    pub(crate) fleet_trucks_required: u32,
    pub(crate) max_cols_across: u32,
    pub(crate) used_length_m: f64,
    pub(crate) used_width_m: f64,
    pub(crate) used_height_m: f64,
    pub(crate) slots: Vec<FloorSlotPlan>,
    pub(crate) zone_spans: Vec<ZoneSpan>,
    pub(crate) bulkhead_positions_m: Vec<f64>,
    pub(crate) recommendations: Vec<String>,
}

/// Resolve the target load, total problem demand, and stop sequence from app context.
pub(crate) fn resolve_target(
    state: &TruckLoadState,
    draft: &Problem,
    bundle: Option<&SolutionBundle>,
) -> TargetResolution {
    let total_problem_demand = draft
        .stores
        .iter()
        .fold(Load::ZERO, |acc, s| acc.saturating_add(s.demand));

    match state.target_source {
        LoadTargetSource::DraftVehicle => {
            let veh = draft.vehicles.get(state.selected_vehicle_idx);
            let label = veh.map_or_else(
                || "No draft vehicle defined".to_string(),
                |v| format!("Draft V{}: {}", state.selected_vehicle_idx, v.name),
            );
            let stops = draft
                .stores
                .iter()
                .enumerate()
                .map(|(i, s)| StopLoadTag {
                    sequence_no: i + 1,
                    store_name: s.name.clone(),
                    demand: s.demand,
                })
                .collect();
            let target_load = veh.map_or(Load::ZERO, |v| v.capacity);
            TargetResolution {
                source_label: label,
                target_load,
                total_problem_demand,
                stops,
            }
        }
        LoadTargetSource::DraftTotalDemand => {
            let stops = draft
                .stores
                .iter()
                .enumerate()
                .map(|(i, s)| StopLoadTag {
                    sequence_no: i + 1,
                    store_name: s.name.clone(),
                    demand: s.demand,
                })
                .collect();
            TargetResolution {
                source_label: format!("All Draft Stores ({} stops)", draft.stores.len()),
                target_load: total_problem_demand,
                total_problem_demand,
                stops,
            }
        }
        LoadTargetSource::SolvedRoute => {
            if let Some(b) = bundle {
                let route_idx = state
                    .selected_route_idx
                    .min(b.solution.routes.len().saturating_sub(1));
                if let Some(route) = b.solution.routes.get(route_idx) {
                    return resolve_route_target(&b.problem, route_idx, route);
                }
            }
            TargetResolution {
                source_label: "No solved route loaded (showing custom load)".to_string(),
                target_load: state.custom_load,
                total_problem_demand,
                stops: Vec::new(),
            }
        }
        LoadTargetSource::CustomLoad => TargetResolution {
            source_label: "Custom What-If Test Load".to_string(),
            target_load: state.custom_load,
            total_problem_demand,
            stops: Vec::new(),
        },
    }
}

fn resolve_route_target(
    problem: &Problem,
    route_idx: usize,
    route: &RoutePlan,
) -> TargetResolution {
    let total_problem_demand = problem
        .stores
        .iter()
        .fold(Load::ZERO, |acc, s| acc.saturating_add(s.demand));
    let mut route_load = Load::ZERO;
    let mut stops = Vec::new();
    let mut seq = 0usize;
    for step in &route.steps {
        if let Node::Store(store_id) = step.node
            && let Some(store) = problem.store(store_id)
        {
            seq += 1;
            route_load = route_load.saturating_add(store.demand);
            stops.push(StopLoadTag {
                sequence_no: seq,
                store_name: store.name.clone(),
                demand: store.demand,
            });
        }
    }
    let vname = vehicle_name(problem, route);
    TargetResolution {
        source_label: format!("Route {} ({vname}, {} stops)", route_idx + 1, stops.len()),
        target_load: route_load,
        total_problem_demand,
        stops,
    }
}

/// Core capacity optimization and 3D slot-layout engine.
pub(crate) fn compute_truck_capacity_plan(
    truck: &TruckDimensions,
    unit: &HandlingUnitSpec,
    split_mode: CompartmentSplitMode,
    target: &TargetResolution,
    preview_max_fill: bool,
) -> TruckCapacityPlan {
    let bay_l = truck.bay_length_m.max(0.0);
    let bay_w = truck.bay_width_m.max(0.0);
    let bay_h = truck.bay_height_m.max(0.0);
    let door_h = if truck.door_clearance_m > 0.0 {
        truck.door_clearance_m
    } else {
        bay_h
    };
    let effective_ceiling_m = bay_h.min(door_h).max(0.0);

    let unit_l = unit.length_m.max(0.01);
    let unit_w = unit.width_m.max(0.01);
    let unit_h = unit.height_m.max(0.01);
    let unit_vol = unit.effective_volume_m3();

    // Determine compartment weights/shares first so active bulkheads are known.
    let shares = compute_compartment_shares(split_mode, target.target_load);
    let active_zones = [
        shares.frozen > 0.0 || target.target_load.frozen > 0,
        shares.chilled > 0.0 || target.target_load.chilled > 0,
        shares.ambient > 0.0 || target.target_load.ambient > 0,
    ]
    .into_iter()
    .filter(|&b| b)
    .count()
    .max(1);
    let active_bulkheads = active_zones.saturating_sub(1);
    let bulkhead_loss_m = active_bulkheads as f64 * truck.bulkhead_thickness_m.max(0.0);
    let usable_length_m = (bay_l - bulkhead_loss_m).max(0.0);

    // 1. Optimal floor packing across standard, 90°-rotated, and mixed-row layouts.
    let std_rows = floor_count(usable_length_m, unit_l);
    let std_cols = floor_count(bay_w, unit_w);
    let std_total = std_rows * std_cols;

    let (orientation, rotation_gain_units) = if unit.allow_rotation {
        let rot_rows = floor_count(usable_length_m, unit_w);
        let rot_cols = floor_count(bay_w, unit_l);
        let rot_total = rot_rows * rot_cols;

        let mut best_choice = if rot_total > std_total {
            FloorOrientationChoice::Rotated90 {
                rows: rot_rows,
                cols: rot_cols,
            }
        } else {
            FloorOrientationChoice::Standard {
                rows: std_rows,
                cols: std_cols,
            }
        };

        if (unit_l - unit_w).abs() > 1e-4 && std_cols > 0 && rot_cols > 0 {
            for r_std in 1..=std_rows {
                let rem_len = usable_length_m - f64::from(r_std) * unit_l;
                let r_rot = floor_count(rem_len, unit_w);
                let mixed_total = r_std * std_cols + r_rot * rot_cols;
                if r_rot > 0 && mixed_total > best_choice.units_per_layer() {
                    best_choice = FloorOrientationChoice::Mixed {
                        std_rows: r_std,
                        std_cols,
                        rot_rows: r_rot,
                        rot_cols,
                    };
                }
            }
        }

        let gain = best_choice.units_per_layer().saturating_sub(std_total);
        (best_choice, gain)
    } else {
        (
            FloorOrientationChoice::Standard {
                rows: std_rows,
                cols: std_cols,
            },
            0,
        )
    };

    let units_per_layer = orientation.units_per_layer();
    let max_layers_by_ceiling = floor_count(effective_ceiling_m, unit_h);
    let stack_layers = max_layers_by_ceiling.min(unit.max_stack_layers.max(1));
    let geometric_max_units = units_per_layer.saturating_mul(stack_layers);

    let gross_bay_volume_m3 = truck.gross_volume_m3();
    let usable_bay_volume_m3 = if truck.use_volume_override {
        truck.volume_override_m3.max(0.0)
    } else {
        usable_length_m
            * bay_w
            * effective_ceiling_m
            * (truck.usable_volume_efficiency_pct.clamp(1.0, 100.0) / 100.0)
    };

    let volume_max_units = floor_count(usable_bay_volume_m3, unit_vol);
    let avg_unit_weight_kg = shares.frozen * unit.weight_for(CompartmentKind::Frozen)
        + shares.chilled * unit.weight_for(CompartmentKind::Chilled)
        + shares.ambient * unit.weight_for(CompartmentKind::Ambient);
    let weight_max_units = if avg_unit_weight_kg > 0.0 {
        floor_count(truck.max_payload_kg.max(0.0), avg_unit_weight_kg)
    } else {
        0
    };

    let effective_max_units = geometric_max_units
        .min(volume_max_units)
        .min(weight_max_units);

    let bottleneck = determine_bottleneck(&BottleneckInput {
        bay_l,
        bay_w,
        bay_h,
        door_h,
        unit_h,
        max_stack_layers: unit.max_stack_layers.max(1),
        units_per_layer,
        stack_layers,
        geometric_max: geometric_max_units,
        volume_max: volume_max_units,
        weight_max: weight_max_units,
        effective_max: effective_max_units,
    });

    let recommended_capacity = allocate_largest_remainder(effective_max_units, shares);

    // Decide what load fills the visual truck slots.
    let desired_fill = if preview_max_fill {
        recommended_capacity
    } else {
        target.target_load
    };

    // Fit desired fill within effective max capacity compartment-by-compartment
    // (or cap proportionally if previewing custom target without overflow hiding).
    let (displayed_load, overflow_load) = cap_load_to_capacity(desired_fill, recommended_capacity);

    // Build geometric floor rows and slots.
    let (slots, zone_spans, bulkhead_positions_m, used_length_m, used_width_m, max_cols_across) =
        build_floor_slots(&FloorSlotInput {
            orientation,
            unit_l,
            unit_w,
            bay_w,
            bulkhead_thickness_m: truck.bulkhead_thickness_m.max(0.0),
            stack_layers: stack_layers.max(1),
            effective_max_units,
            recommended_capacity,
            displayed_load,
            unit,
            stops: &target.stops,
        });

    let loaded_units = displayed_load.total() as f64;
    let loaded_volume_m3 = loaded_units * unit_vol;
    let dead_volume_m3 = (gross_bay_volume_m3 - loaded_volume_m3).max(0.0);
    let cube_utilization_pct = pct(loaded_volume_m3, gross_bay_volume_m3);
    let usable_cube_utilization_pct = pct(loaded_volume_m3, usable_bay_volume_m3);

    let occupied_floor_m2 = slots
        .iter()
        .filter(|s| s.status == LoadedSlotStatus::Loaded)
        .map(|s| s.slot_length_m * s.slot_width_m)
        .sum::<f64>();
    let floor_utilization_pct = pct(occupied_floor_m2, bay_l * bay_w);

    let loaded_payload_kg = f64::from(displayed_load.ambient)
        * unit.weight_for(CompartmentKind::Ambient)
        + f64::from(displayed_load.chilled) * unit.weight_for(CompartmentKind::Chilled)
        + f64::from(displayed_load.frozen) * unit.weight_for(CompartmentKind::Frozen);
    let weight_utilization_pct = pct(loaded_payload_kg, truck.max_payload_kg);
    let total_gvm_tonnes = (truck.tare_weight_kg.max(0.0) + loaded_payload_kg) / 1000.0;

    let max_layers_used = slots
        .iter()
        .filter(|s| s.status == LoadedSlotStatus::Loaded)
        .map(|s| s.loaded_layers)
        .max()
        .unwrap_or(0);
    let used_height_m = f64::from(max_layers_used) * unit_h;

    let (cog_from_front_pct, axle_balance) = compute_cog_balance(&slots, bay_l);

    let fleet_trucks_required = compute_fleet_trucks_needed(
        target.total_problem_demand,
        effective_max_units,
        recommended_capacity,
        split_mode,
    );

    let recommendations = build_recommendations(&RecommendationCtx {
        truck,
        unit,
        split_mode,
        usable_length_m,
        effective_ceiling_m,
        orientation,
        rotation_gain_units,
        std_total,
        units_per_layer,
        stack_layers,
        geometric_max_units,
        volume_max_units,
        weight_max_units,
        effective_max_units,
        bottleneck,
        cube_utilization_pct,
        weight_utilization_pct,
        overflow_load,
        total_problem_demand: target.total_problem_demand,
        fleet_trucks_required,
    });

    TruckCapacityPlan {
        target_label: target.source_label.clone(),
        active_zones,
        active_bulkheads,
        usable_length_m,
        effective_ceiling_m,
        gross_bay_volume_m3,
        usable_bay_volume_m3,
        unit_volume_m3: unit_vol,
        orientation,
        rotation_gain_units,
        units_per_layer,
        stack_layers,
        geometric_max_units,
        volume_max_units,
        weight_max_units,
        effective_max_units,
        bottleneck,
        recommended_capacity,
        displayed_load,
        target_load: target.target_load,
        overflow_load,
        cube_utilization_pct,
        usable_cube_utilization_pct,
        floor_utilization_pct,
        weight_utilization_pct,
        loaded_volume_m3,
        dead_volume_m3,
        loaded_payload_kg,
        total_gvm_tonnes,
        cog_from_front_pct,
        axle_balance,
        fleet_trucks_required,
        max_cols_across,
        used_length_m,
        used_width_m,
        used_height_m,
        slots,
        zone_spans,
        bulkhead_positions_m,
        recommendations,
    }
}

#[derive(Clone, Copy)]
struct CompartmentShares {
    frozen: f64,
    chilled: f64,
    ambient: f64,
}

fn compute_compartment_shares(
    split_mode: CompartmentSplitMode,
    target_load: Load,
) -> CompartmentShares {
    match split_mode {
        CompartmentSplitMode::SingleZoneAmbient => CompartmentShares {
            frozen: 0.0,
            chilled: 0.0,
            ambient: 1.0,
        },
        CompartmentSplitMode::ManualPercentages {
            ambient_pct,
            chilled_pct,
            frozen_pct,
        } => {
            let a = ambient_pct.max(0.0);
            let c = chilled_pct.max(0.0);
            let f = frozen_pct.max(0.0);
            let sum = a + c + f;
            if sum > EPS {
                CompartmentShares {
                    frozen: f / sum,
                    chilled: c / sum,
                    ambient: a / sum,
                }
            } else {
                CompartmentShares {
                    frozen: 0.0,
                    chilled: 0.0,
                    ambient: 1.0,
                }
            }
        }
        CompartmentSplitMode::MatchTargetLoad => {
            let total = target_load.total() as f64;
            if total > 0.0 {
                CompartmentShares {
                    frozen: f64::from(target_load.frozen) / total,
                    chilled: f64::from(target_load.chilled) / total,
                    ambient: f64::from(target_load.ambient) / total,
                }
            } else {
                // Default 3-zone supermarket grocery balance when target is zero.
                CompartmentShares {
                    frozen: 0.15,
                    chilled: 0.30,
                    ambient: 0.55,
                }
            }
        }
    }
}

fn floor_count(available: f64, item: f64) -> u32 {
    if available <= 0.0 || item <= 0.0 {
        0
    } else {
        ((available + EPS) / item).floor().max(0.0) as u32
    }
}

fn pct(part: f64, total: f64) -> f64 {
    if total > EPS {
        (part / total * 100.0).clamp(0.0, 999.9)
    } else {
        0.0
    }
}

fn allocate_largest_remainder(total: u32, shares: CompartmentShares) -> Load {
    if total == 0 {
        return Load::ZERO;
    }
    let weights = [shares.frozen, shares.chilled, shares.ambient];
    let exact: [f64; 3] = weights.map(|w| w * f64::from(total));
    let mut base = exact.map(|v| v.floor() as u32);
    let mut remainder_units = total.saturating_sub(base[0] + base[1] + base[2]);

    let mut order = [
        (0usize, exact[0] - f64::from(base[0])),
        (1usize, exact[1] - f64::from(base[1])),
        (2usize, exact[2] - f64::from(base[2])),
    ];
    order.sort_by(|a, b| b.1.total_cmp(&a.1));

    for (idx, _) in order {
        if remainder_units == 0 {
            break;
        }
        if weights[idx] > 0.0 {
            base[idx] += 1;
            remainder_units -= 1;
        }
    }

    Load::new(base[2], base[1], base[0])
}

fn cap_load_to_capacity(desired: Load, capacity: Load) -> (Load, Load) {
    let fitted = Load::new(
        desired.ambient.min(capacity.ambient),
        desired.chilled.min(capacity.chilled),
        desired.frozen.min(capacity.frozen),
    );
    let overflow = Load::new(
        desired.ambient.saturating_sub(capacity.ambient),
        desired.chilled.saturating_sub(capacity.chilled),
        desired.frozen.saturating_sub(capacity.frozen),
    );
    (fitted, overflow)
}

struct BottleneckInput {
    bay_l: f64,
    bay_w: f64,
    bay_h: f64,
    door_h: f64,
    unit_h: f64,
    max_stack_layers: u32,
    units_per_layer: u32,
    stack_layers: u32,
    geometric_max: u32,
    volume_max: u32,
    weight_max: u32,
    effective_max: u32,
}

fn determine_bottleneck(input: &BottleneckInput) -> BottleneckKind {
    if input.bay_l <= 0.0
        || input.bay_w <= 0.0
        || input.bay_h <= 0.0
        || input.units_per_layer == 0
        || input.stack_layers == 0
    {
        return BottleneckKind::Unconstrained;
    }
    if input.weight_max == input.effective_max
        && input.weight_max < input.geometric_max
        && input.weight_max <= input.volume_max
    {
        return BottleneckKind::PayloadWeightLimit;
    }
    if input.volume_max == input.effective_max && input.volume_max < input.geometric_max {
        return BottleneckKind::UsableVolume;
    }
    // Check if rear door clearance alone blocked another vertical layer.
    let bay_layers = floor_count(input.bay_h, input.unit_h);
    let door_layers = floor_count(input.door_h, input.unit_h);
    if door_layers < bay_layers
        && door_layers < input.max_stack_layers
        && input.effective_max == input.geometric_max
    {
        return BottleneckKind::DoorClearance;
    }
    if bay_layers < input.max_stack_layers && input.effective_max == input.geometric_max {
        return BottleneckKind::CeilingOrStackHeight;
    }
    BottleneckKind::FloorFootprint
}

struct FloorSlotInput<'a> {
    orientation: FloorOrientationChoice,
    unit_l: f64,
    unit_w: f64,
    bay_w: f64,
    bulkhead_thickness_m: f64,
    stack_layers: u32,
    effective_max_units: u32,
    recommended_capacity: Load,
    displayed_load: Load,
    unit: &'a HandlingUnitSpec,
    stops: &'a [StopLoadTag],
}

fn build_floor_slots(
    input: &FloorSlotInput<'_>,
) -> (Vec<FloorSlotPlan>, Vec<ZoneSpan>, Vec<f64>, f64, f64, u32) {
    struct RowSpec {
        length_m: f64,
        cols: u32,
        slot_width_m: f64,
        rotated: bool,
    }

    let mut rows = Vec::new();
    match input.orientation {
        FloorOrientationChoice::Standard { rows: r, cols } => {
            for _ in 0..r {
                rows.push(RowSpec {
                    length_m: input.unit_l,
                    cols,
                    slot_width_m: input.unit_w,
                    rotated: false,
                });
            }
        }
        FloorOrientationChoice::Rotated90 { rows: r, cols } => {
            for _ in 0..r {
                rows.push(RowSpec {
                    length_m: input.unit_w,
                    cols,
                    slot_width_m: input.unit_l,
                    rotated: true,
                });
            }
        }
        FloorOrientationChoice::Mixed {
            std_rows,
            std_cols,
            rot_rows,
            rot_cols,
        } => {
            for _ in 0..std_rows {
                rows.push(RowSpec {
                    length_m: input.unit_l,
                    cols: std_cols,
                    slot_width_m: input.unit_w,
                    rotated: false,
                });
            }
            for _ in 0..rot_rows {
                rows.push(RowSpec {
                    length_m: input.unit_w,
                    cols: rot_cols,
                    slot_width_m: input.unit_l,
                    rotated: true,
                });
            }
        }
    }

    let total_floor_positions: usize = rows.iter().map(|r| r.cols as usize).sum();
    if total_floor_positions == 0 {
        return (Vec::new(), Vec::new(), Vec::new(), 0.0, 0.0, 0);
    }

    // Build per-unit queues for each compartment (Frozen front -> Chilled middle -> Ambient rear).
    // Stops are queued in reverse order (LIFO: highest stop number goes closest to the cab,
    // first stop S1 sits closest to the rear doors).
    let frozen_stop_units = build_lifo_unit_queue(input.stops, CompartmentKind::Frozen);
    let chilled_stop_units = build_lifo_unit_queue(input.stops, CompartmentKind::Chilled);
    let ambient_stop_units = build_lifo_unit_queue(input.stops, CompartmentKind::Ambient);

    // Determine exact slot allocations (how many floor slots each compartment receives).
    let stack = input.stack_layers.max(1);
    let needed_slots_frozen = input.recommended_capacity.frozen.div_ceil(stack);
    let needed_slots_chilled = input.recommended_capacity.chilled.div_ceil(stack);
    let needed_slots_ambient = input.recommended_capacity.ambient.div_ceil(stack);
    let unlocked_floor_slots = input.effective_max_units.div_ceil(stack) as usize;

    let mut slot_compartments = Vec::with_capacity(total_floor_positions);
    for _ in 0..needed_slots_frozen {
        if slot_compartments.len() < unlocked_floor_slots {
            slot_compartments.push((CompartmentKind::Frozen, true));
        }
    }
    for _ in 0..needed_slots_chilled {
        if slot_compartments.len() < unlocked_floor_slots {
            slot_compartments.push((CompartmentKind::Chilled, true));
        }
    }
    for _ in 0..needed_slots_ambient {
        if slot_compartments.len() < unlocked_floor_slots {
            slot_compartments.push((CompartmentKind::Ambient, true));
        }
    }
    // Fill any remaining unlocked floor positions with the largest active compartment.
    let fallback_comp = dominant_compartment(input.recommended_capacity);
    while slot_compartments.len() < unlocked_floor_slots.min(total_floor_positions) {
        slot_compartments.push((fallback_comp, true));
    }
    // Any remaining geometric positions beyond effective_max_units are locked by weight/volume.
    while slot_compartments.len() < total_floor_positions {
        slot_compartments.push((fallback_comp, false));
    }

    let mut remaining_to_load = [
        input.displayed_load.frozen,
        input.displayed_load.chilled,
        input.displayed_load.ambient,
    ];
    let mut stop_cursors = [0usize, 0usize, 0usize];
    let stop_queues = [&frozen_stop_units, &chilled_stop_units, &ambient_stop_units];

    let mut slots = Vec::with_capacity(total_floor_positions);
    let mut bulkhead_positions_m = Vec::new();
    let mut zone_bounds: BTreeMap<usize, (f64, f64, u32)> = BTreeMap::new();

    let mut cursor_x = 0.0_f64;
    let mut global_idx = 0usize;
    let mut prev_zone: Option<CompartmentKind> = None;
    let mut used_width_m = 0.0_f64;
    let mut max_cols_across = 0u32;

    for (r_idx, row) in rows.iter().enumerate() {
        max_cols_across = max_cols_across.max(row.cols);
        let row_width = f64::from(row.cols) * row.slot_width_m;
        used_width_m = used_width_m.max(row_width);
        let y_margin = ((input.bay_w - row_width) * 0.5).max(0.0);

        let row_primary_comp = slot_compartments
            .get(global_idx)
            .map_or(fallback_comp, |(c, _)| *c);
        if let Some(prev) = prev_zone
            && prev != row_primary_comp
        {
            bulkhead_positions_m.push(cursor_x);
            cursor_x += input.bulkhead_thickness_m;
        }
        prev_zone = Some(row_primary_comp);

        for c_idx in 0..row.cols {
            let (compartment, is_unlocked) = slot_compartments[global_idx];
            global_idx += 1;

            let comp_idx = match compartment {
                CompartmentKind::Frozen => 0,
                CompartmentKind::Chilled => 1,
                CompartmentKind::Ambient => 2,
            };

            let status = if !is_unlocked {
                LoadedSlotStatus::WeightOrVolumeLocked
            } else if remaining_to_load[comp_idx] > 0 {
                LoadedSlotStatus::Loaded
            } else {
                LoadedSlotStatus::EmptyCapacity
            };

            let loaded_layers = if status == LoadedSlotStatus::Loaded {
                let take = remaining_to_load[comp_idx].min(stack);
                remaining_to_load[comp_idx] -= take;
                take
            } else {
                0
            };

            let mut stop_badges = Vec::new();
            let mut stop_details = Vec::new();
            let queue = stop_queues[comp_idx];
            for _ in 0..loaded_layers {
                if let Some((seq, name)) = queue.get(stop_cursors[comp_idx]) {
                    stop_cursors[comp_idx] += 1;
                    let badge = format!("S{seq}");
                    if !stop_badges.contains(&badge) {
                        stop_badges.push(badge);
                        stop_details.push(format!("S{seq} ({name})"));
                    }
                }
            }

            let y_start_m = y_margin + f64::from(c_idx) * row.slot_width_m;
            let unit_wt = input.unit.weight_for(compartment);
            let slot_weight_kg = f64::from(loaded_layers) * unit_wt;

            let status_desc = match status {
                LoadedSlotStatus::Loaded => {
                    format!("Loaded: {loaded_layers}/{stack} units ({slot_weight_kg:.0} kg)")
                }
                LoadedSlotStatus::EmptyCapacity => {
                    format!("Available slot (up to {stack} units)")
                }
                LoadedSlotStatus::WeightOrVolumeLocked => {
                    "Locked by Payload Weight / Usable Volume limit".to_string()
                }
            };
            let orient_desc = if row.rotated {
                "Rotated 90°"
            } else {
                "Standard"
            };
            let stop_line = if stop_details.is_empty() {
                String::new()
            } else {
                format!("\nLIFO Stops: {}", stop_details.join(", "))
            };
            let tooltip = format!(
                "Slot R{}·C{} [{}] ({orient_desc})\n{:.2}m × {:.2}m footprint\n{status_desc}{stop_line}",
                r_idx + 1,
                c_idx + 1,
                compartment.label(),
                row.length_m,
                row.slot_width_m,
            );

            let entry =
                zone_bounds
                    .entry(comp_idx)
                    .or_insert((cursor_x, cursor_x + row.length_m, 0));
            entry.0 = entry.0.min(cursor_x);
            entry.1 = entry.1.max(cursor_x + row.length_m);
            entry.2 += loaded_layers;

            slots.push(FloorSlotPlan {
                row: r_idx as u32 + 1,
                col: c_idx + 1,
                x_start_m: cursor_x,
                y_start_m,
                slot_length_m: row.length_m,
                slot_width_m: row.slot_width_m,
                rotated: row.rotated,
                compartment,
                status,
                loaded_layers,
                max_layers: stack,
                slot_weight_kg,
                stop_badges,
                tooltip,
            });
        }

        cursor_x += row.length_m;
    }

    let mut zone_spans = Vec::new();
    for (idx, comp) in [
        (0usize, CompartmentKind::Frozen),
        (1usize, CompartmentKind::Chilled),
        (2usize, CompartmentKind::Ambient),
    ] {
        if let Some((xs, xe, loaded_u)) = zone_bounds.get(&idx) {
            let cap_u = match comp {
                CompartmentKind::Frozen => input.recommended_capacity.frozen,
                CompartmentKind::Chilled => input.recommended_capacity.chilled,
                CompartmentKind::Ambient => input.recommended_capacity.ambient,
            };
            zone_spans.push(ZoneSpan {
                compartment: comp,
                x_start_m: *xs,
                x_end_m: *xe,
                capacity_units: cap_u,
                loaded_units: *loaded_u,
            });
        }
    }

    (
        slots,
        zone_spans,
        bulkhead_positions_m,
        cursor_x,
        used_width_m,
        max_cols_across,
    )
}

fn dominant_compartment(load: Load) -> CompartmentKind {
    if load.ambient >= load.chilled && load.ambient >= load.frozen {
        CompartmentKind::Ambient
    } else if load.chilled >= load.frozen {
        CompartmentKind::Chilled
    } else {
        CompartmentKind::Frozen
    }
}

/// LIFO order: last delivery goes into the bay first (closest to cab/front of zone),
/// first delivery (`S1`) goes last (nearest rear doors / easy unload).
fn build_lifo_unit_queue(
    stops: &[StopLoadTag],
    compartment: CompartmentKind,
) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for stop in stops.iter().rev() {
        let count = match compartment {
            CompartmentKind::Frozen => stop.demand.frozen,
            CompartmentKind::Chilled => stop.demand.chilled,
            CompartmentKind::Ambient => stop.demand.ambient,
        };
        for _ in 0..count {
            out.push((stop.sequence_no, stop.store_name.clone()));
        }
    }
    out
}

fn compute_cog_balance(slots: &[FloorSlotPlan], bay_length_m: f64) -> (f64, AxleBalanceStatus) {
    if bay_length_m <= EPS {
        return (50.0, AxleBalanceStatus::Unloaded);
    }
    let mut total_weight = 0.0_f64;
    let mut weighted_x = 0.0_f64;
    for slot in slots {
        if slot.status == LoadedSlotStatus::Loaded && slot.slot_weight_kg > 0.0 {
            let center_x = slot.x_start_m + slot.slot_length_m * 0.5;
            total_weight += slot.slot_weight_kg;
            weighted_x += center_x * slot.slot_weight_kg;
        }
    }
    if total_weight <= EPS {
        return (50.0, AxleBalanceStatus::Unloaded);
    }
    let cog_pct = (weighted_x / total_weight / bay_length_m * 100.0).clamp(0.0, 100.0);
    let balance = if cog_pct < 42.0 {
        AxleBalanceStatus::FrontHeavy
    } else if cog_pct > 62.0 {
        AxleBalanceStatus::RearHeavy
    } else {
        AxleBalanceStatus::Balanced
    };
    (cog_pct, balance)
}

fn compute_fleet_trucks_needed(
    total_demand: Load,
    effective_max_units: u32,
    recommended_capacity: Load,
    split_mode: CompartmentSplitMode,
) -> u32 {
    if total_demand.total() == 0 {
        return 0;
    }
    if effective_max_units == 0 {
        return u32::MAX;
    }
    match split_mode {
        CompartmentSplitMode::MatchTargetLoad => {
            let total_req = (total_demand.total() as u32).div_ceil(effective_max_units);
            let a_req = if recommended_capacity.ambient > 0 {
                total_demand.ambient.div_ceil(recommended_capacity.ambient)
            } else {
                0
            };
            let c_req = if recommended_capacity.chilled > 0 {
                total_demand.chilled.div_ceil(recommended_capacity.chilled)
            } else {
                0
            };
            let f_req = if recommended_capacity.frozen > 0 {
                total_demand.frozen.div_ceil(recommended_capacity.frozen)
            } else {
                0
            };
            total_req.max(a_req).max(c_req).max(f_req)
        }
        CompartmentSplitMode::ManualPercentages { .. } => {
            let mut needed = (total_demand.total() as u32).div_ceil(effective_max_units);
            if total_demand.ambient > 0 && recommended_capacity.ambient > 0 {
                needed = needed.max(total_demand.ambient.div_ceil(recommended_capacity.ambient));
            }
            if total_demand.chilled > 0 && recommended_capacity.chilled > 0 {
                needed = needed.max(total_demand.chilled.div_ceil(recommended_capacity.chilled));
            }
            if total_demand.frozen > 0 && recommended_capacity.frozen > 0 {
                needed = needed.max(total_demand.frozen.div_ceil(recommended_capacity.frozen));
            }
            needed
        }
        CompartmentSplitMode::SingleZoneAmbient => {
            (total_demand.total() as u32).div_ceil(effective_max_units)
        }
    }
}

struct RecommendationCtx<'a> {
    truck: &'a TruckDimensions,
    unit: &'a HandlingUnitSpec,
    split_mode: CompartmentSplitMode,
    usable_length_m: f64,
    effective_ceiling_m: f64,
    orientation: FloorOrientationChoice,
    rotation_gain_units: u32,
    std_total: u32,
    units_per_layer: u32,
    stack_layers: u32,
    geometric_max_units: u32,
    volume_max_units: u32,
    weight_max_units: u32,
    effective_max_units: u32,
    bottleneck: BottleneckKind,
    cube_utilization_pct: f64,
    weight_utilization_pct: f64,
    overflow_load: Load,
    total_problem_demand: Load,
    fleet_trucks_required: u32,
}

fn build_recommendations(ctx: &RecommendationCtx<'_>) -> Vec<String> {
    let mut tips = Vec::new();
    let unit_l = ctx.unit.length_m.max(0.01);
    let unit_w = ctx.unit.width_m.max(0.01);
    let unit_h = ctx.unit.height_m.max(0.01);

    if ctx.effective_max_units == 0 {
        tips.push(
            "Handling unit is larger than internal truck dimensions — increase bay size or reduce unit dimensions."
                .to_string(),
        );
        return tips;
    }

    if ctx.rotation_gain_units > 0 {
        tips.push(format!(
            "Floor orientation optimization ({}) added +{} floor positions (+{} total units) over unrotated lengthwise loading.",
            ctx.orientation.label(),
            ctx.rotation_gain_units,
            ctx.rotation_gain_units * ctx.stack_layers
        ));
    } else if !ctx.unit.allow_rotation {
        let rot_r = floor_count(ctx.usable_length_m, unit_w);
        let rot_c = floor_count(ctx.truck.bay_width_m, unit_l);
        let rot_total = rot_r * rot_c;
        if rot_total > ctx.std_total {
            tips.push(format!(
                "Tip: Enabling 90° rotation would raise floor capacity from {} to {} units per layer (+{} total capacity).",
                ctx.std_total,
                rot_total,
                (rot_total - ctx.std_total) * ctx.stack_layers
            ));
        }
    }

    if ctx.bottleneck == BottleneckKind::PayloadWeightLimit {
        tips.push(format!(
            "Weighing out before cubing out: payload limit caps the truck at {} units (floor+stack could fit {}). Reducing unit weight or upgrading GVM unlocks +{} units.",
            ctx.weight_max_units,
            ctx.geometric_max_units,
            ctx.geometric_max_units.saturating_sub(ctx.weight_max_units)
        ));
    } else if ctx.bottleneck == BottleneckKind::UsableVolume {
        tips.push(format!(
            "Volume limit caps capacity at {} units vs {} geometric slots — adjust usable volume efficiency or custom volume if units nest.",
            ctx.volume_max_units, ctx.geometric_max_units
        ));
    }

    // Check next width / length / height threshold needed to unlock more capacity.
    let bay_w = ctx.truck.bay_width_m;
    let std_cols = floor_count(bay_w, unit_w);
    let next_w_std = f64::from(std_cols + 1) * unit_w - bay_w;
    let next_w_rot = if ctx.unit.allow_rotation {
        let rc = floor_count(bay_w, unit_l);
        Some(f64::from(rc + 1) * unit_l - bay_w)
    } else {
        None
    };
    let min_width_delta = next_w_rot.map_or(next_w_std, |r| next_w_std.min(r));
    if min_width_delta > 0.0 && min_width_delta <= 0.45 {
        tips.push(format!(
            "Width margin: +{min_width_delta:.2} m internal width unlocks an extra floor column across the bay."
        ));
    }

    if ctx.truck.door_clearance_m > 0.0
        && ctx.truck.door_clearance_m < ctx.truck.bay_height_m
        && floor_count(ctx.truck.door_clearance_m, unit_h)
            < floor_count(ctx.truck.bay_height_m, unit_h)
        && ctx.stack_layers < ctx.unit.max_stack_layers
    {
        tips.push(format!(
            "Door clearance ({:.2} m) is blocking vertical layer #{} even though internal ceiling is {:.2} m (+{:.2} m door height adds +{} units).",
            ctx.truck.door_clearance_m,
            ctx.stack_layers + 1,
            ctx.truck.bay_height_m,
            (f64::from(ctx.stack_layers + 1) * unit_h - ctx.effective_ceiling_m).max(0.01),
            ctx.units_per_layer
        ));
    } else if ctx.stack_layers < ctx.unit.max_stack_layers {
        let next_h_delta = f64::from(ctx.stack_layers + 1) * unit_h - ctx.effective_ceiling_m;
        if next_h_delta > 0.0 && next_h_delta <= 0.50 {
            tips.push(format!(
                "Height margin: +{next_h_delta:.2} m ceiling & door clearance unlocks stack layer #{} (+{} units).",
                ctx.stack_layers + 1,
                ctx.units_per_layer
            ));
        }
    }

    if ctx.overflow_load.total() > 0 {
        tips.push(format!(
            "Target load exceeds compartment capacity by A:{} C:{} F:{} units — add another truck or increase bay dimensions/zone share.",
            ctx.overflow_load.ambient,
            ctx.overflow_load.chilled,
            ctx.overflow_load.frozen
        ));
    }

    if matches!(
        ctx.split_mode,
        CompartmentSplitMode::ManualPercentages { .. }
    ) && (ctx.overflow_load.total() > 0)
    {
        tips.push(
            "Consider switching Compartment Split to 'Match Target Load Ratio' so movable bulkheads align with demand."
                .to_string(),
        );
    }

    if ctx.total_problem_demand.total() > 0 && ctx.fleet_trucks_required != u32::MAX {
        tips.push(format!(
            "Fleet sizing: at {} units max capacity, at least {} truck(s) of this spec are needed for total problem demand ({} units across A:{} C:{} F:{}).",
            ctx.effective_max_units,
            ctx.fleet_trucks_required,
            ctx.total_problem_demand.total(),
            ctx.total_problem_demand.ambient,
            ctx.total_problem_demand.chilled,
            ctx.total_problem_demand.frozen,
        ));
    }

    if ctx.cube_utilization_pct < 55.0 && ctx.weight_utilization_pct < 55.0 {
        tips.push(
            "Both cube and payload utilization are below 55% — consider downsizing to a smaller truck body or consolidating stops."
                .to_string(),
        );
    }

    tips
}

/// Render the **Truck Load** tab and return any requested action for `ConsoleApp`.
pub(crate) fn render_truck_load_tab(
    ui: &mut egui::Ui,
    state: &mut TruckLoadState,
    draft: &Problem,
    bundle: Option<&SolutionBundle>,
) -> Option<TruckLoadAction> {
    let mut action: Option<TruckLoadAction> = None;

    ui.heading("Truck Load & Capacity Optimizer");
    ui.label("Configure internal cargo bay dimensions, volume, payload weight, and handling units to optimize truck fill and push capacities directly to your route problem.");

    if let Some(banner) = &state.status_banner {
        ui.add_space(4.0);
        ui.colored_label(egui::Color32::from_rgb(76, 175, 80), banner);
    }

    // Clamp indices safely against current draft/solution bounds.
    if !draft.vehicles.is_empty() && state.selected_vehicle_idx >= draft.vehicles.len() {
        state.selected_vehicle_idx = 0;
    }
    if let Some(b) = bundle
        && !b.solution.routes.is_empty()
        && state.selected_route_idx >= b.solution.routes.len()
    {
        state.selected_route_idx = 0;
    }

    let target = resolve_target(state, draft, bundle);
    let plan = compute_truck_capacity_plan(
        &state.truck,
        &state.unit,
        state.active_split_mode(),
        &target,
        state.preview_max_fill,
    );

    ui.add_space(6.0);
    render_kpi_summary_strip(ui, &plan);
    ui.add_space(8.0);

    egui::ScrollArea::vertical()
        .id_salt("truck_load_main_scroll")
        .show(ui, |ui| {
            ui.columns(2, |cols| {
                // Left column: Truck specs, Handling unit specs, Load target, Actions.
                render_truck_specs_panel(&mut cols[0], state);
                cols[0].add_space(6.0);
                render_unit_specs_panel(&mut cols[0], state);
                cols[0].add_space(6.0);
                render_target_and_split_panel(
                    &mut cols[0],
                    state,
                    draft,
                    bundle,
                    &plan,
                    &mut action,
                );

                // Right column: Interactive visual truck diagrams + Recommendations.
                render_visual_panel(&mut cols[1], state, &plan);
                cols[1].add_space(6.0);
                render_diagnostics_panel(&mut cols[1], &plan);
            });
        });

    action
}

fn render_kpi_summary_strip(ui: &mut egui::Ui, plan: &TruckCapacityPlan) {
    ui.horizontal_wrapped(|ui| {
        stat_card(
            ui,
            "Max Truck Capacity",
            &format!(
                "{} units (A:{} C:{} F:{})",
                plan.effective_max_units,
                plan.recommended_capacity.ambient,
                plan.recommended_capacity.chilled,
                plan.recommended_capacity.frozen
            ),
        );
        stat_card(ui, "Bottleneck", plan.bottleneck.label());
        stat_card(
            ui,
            "Cube Utilization",
            &format!(
                "{:.1}% ({:.1}/{:.1} m³)",
                plan.cube_utilization_pct, plan.loaded_volume_m3, plan.gross_bay_volume_m3
            ),
        );
        stat_card(
            ui,
            "Floor Footprint",
            &format!(
                "{:.1}% ({} / layer × {}L)",
                plan.floor_utilization_pct, plan.units_per_layer, plan.stack_layers
            ),
        );
        stat_card(
            ui,
            "Payload / GVM",
            &format!(
                "{:.0} kg ({:.1}%) · {:.2}t GVM",
                plan.loaded_payload_kg, plan.weight_utilization_pct, plan.total_gvm_tonnes
            ),
        );
        if plan.fleet_trucks_required > 0 && plan.fleet_trucks_required != u32::MAX {
            stat_card(
                ui,
                "Min Fleet Needed",
                &format!("{} truck(s)", plan.fleet_trucks_required),
            );
        }
    });
}

fn render_truck_specs_panel(ui: &mut egui::Ui, state: &mut TruckLoadState) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong("1. Truck Cargo Bay & Mass Limits");
        ui.add_space(4.0);

        ui.horizontal_wrapped(|ui| {
            ui.label("Vehicle preset:");
            egui::ComboBox::from_id_salt("truck_preset_combo")
                .selected_text(state.truck_preset.label())
                .show_ui(ui, |ui| {
                    for preset in TruckPreset::ALL {
                        if ui
                            .selectable_value(&mut state.truck_preset, preset, preset.label())
                            .clicked()
                            && preset != TruckPreset::Custom
                        {
                            state.truck = preset.dimensions();
                        }
                    }
                });
        });

        let mut custom_edited = false;
        egui::Grid::new("truck_dims_grid")
            .num_columns(4)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Bay Length (m):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.truck.bay_length_m)
                            .speed(0.05)
                            .fixed_decimals(2)
                            .range(1.0..=30.0),
                    )
                    .changed();
                ui.label("Bay Width (m):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.truck.bay_width_m)
                            .speed(0.02)
                            .fixed_decimals(2)
                            .range(0.5..=4.0),
                    )
                    .changed();
                ui.end_row();

                ui.label("Bay Height (m):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.truck.bay_height_m)
                            .speed(0.02)
                            .fixed_decimals(2)
                            .range(0.5..=4.5),
                    )
                    .changed();
                ui.label("Door Clearance (m):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.truck.door_clearance_m)
                            .speed(0.02)
                            .fixed_decimals(2)
                            .range(0.5..=4.5),
                    )
                    .changed();
                ui.end_row();

                ui.label("Max Payload (kg):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.truck.max_payload_kg)
                            .speed(50.0)
                            .fixed_decimals(0)
                            .range(100.0..=60_000.0),
                    )
                    .changed();
                ui.label("Tare Weight (kg):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.truck.tare_weight_kg)
                            .speed(50.0)
                            .fixed_decimals(0)
                            .range(500.0..=40_000.0),
                    )
                    .changed();
                ui.end_row();

                ui.label("External Height (m):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.truck.external_height_m)
                            .speed(0.05)
                            .fixed_decimals(2)
                            .range(1.5..=5.0),
                    )
                    .changed();
                ui.label("Bulkhead Thick (m):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.truck.bulkhead_thickness_m)
                            .speed(0.01)
                            .fixed_decimals(2)
                            .range(0.0..=0.35),
                    )
                    .changed();
                ui.end_row();

                ui.label("Packing Factor (%):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.truck.usable_volume_efficiency_pct)
                            .speed(1.0)
                            .fixed_decimals(0)
                            .range(50.0..=100.0),
                    )
                    .changed();
                ui.label("Calc Gross Vol:");
                ui.strong(format!("{:.2} m³", state.truck.gross_volume_m3()));
                ui.end_row();
            });

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut state.truck.use_volume_override,
                "Override Usable Volume (m³):",
            );
            ui.add_enabled(
                state.truck.use_volume_override,
                egui::DragValue::new(&mut state.truck.volume_override_m3)
                    .speed(0.25)
                    .fixed_decimals(2)
                    .range(0.5..=250.0),
            );
            ui.weak(format!("Max GVM: {:.2} t", state.truck.max_gvm_tonnes()));
        });

        if custom_edited {
            state.truck_preset = TruckPreset::Custom;
        }
    });
}

fn render_unit_specs_panel(ui: &mut egui::Ui, state: &mut TruckLoadState) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong("2. Handling Unit (Cage / Pallet / Crate)");
        ui.add_space(4.0);

        ui.horizontal_wrapped(|ui| {
            ui.label("Unit preset:");
            egui::ComboBox::from_id_salt("unit_preset_combo")
                .selected_text(state.unit_preset.label())
                .show_ui(ui, |ui| {
                    for preset in UnitPreset::ALL {
                        if ui
                            .selectable_value(&mut state.unit_preset, preset, preset.label())
                            .clicked()
                            && preset != UnitPreset::Custom
                        {
                            state.unit = preset.spec();
                        }
                    }
                });
        });

        let mut custom_edited = false;
        egui::Grid::new("unit_dims_grid")
            .num_columns(4)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Unit Length (m):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.unit.length_m)
                            .speed(0.01)
                            .fixed_decimals(3)
                            .range(0.10..=3.0),
                    )
                    .changed();
                ui.label("Unit Width (m):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.unit.width_m)
                            .speed(0.01)
                            .fixed_decimals(3)
                            .range(0.10..=2.5),
                    )
                    .changed();
                ui.end_row();

                ui.label("Unit Height (m):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.unit.height_m)
                            .speed(0.01)
                            .fixed_decimals(2)
                            .range(0.10..=3.0),
                    )
                    .changed();
                ui.label("Max Stack Layers:");
                custom_edited |= ui
                    .add(egui::DragValue::new(&mut state.unit.max_stack_layers).range(1..=12))
                    .changed();
                ui.end_row();

                ui.label("Ambient Wt (kg):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.unit.ambient_weight_kg)
                            .speed(5.0)
                            .fixed_decimals(0)
                            .range(1.0..=3_000.0),
                    )
                    .changed();
                ui.label("Chilled Wt (kg):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.unit.chilled_weight_kg)
                            .speed(5.0)
                            .fixed_decimals(0)
                            .range(1.0..=3_000.0),
                    )
                    .changed();
                ui.end_row();

                ui.label("Frozen Wt (kg):");
                custom_edited |= ui
                    .add(
                        egui::DragValue::new(&mut state.unit.frozen_weight_kg)
                            .speed(5.0)
                            .fixed_decimals(0)
                            .range(1.0..=3_000.0),
                    )
                    .changed();
                ui.label("Unit Vol (L×W×H):");
                ui.strong(format!("{:.3} m³", state.unit.effective_volume_m3()));
                ui.end_row();
            });

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut state.unit.allow_rotation,
                "Allow 90° Floor Rotation (Hybrid Packing)",
            );
            ui.checkbox(&mut state.unit.use_unit_volume_override, "Custom Vol (m³):");
            ui.add_enabled(
                state.unit.use_unit_volume_override,
                egui::DragValue::new(&mut state.unit.unit_volume_override_m3)
                    .speed(0.02)
                    .fixed_decimals(3)
                    .range(0.005..=20.0),
            );
        });

        if custom_edited {
            state.unit_preset = UnitPreset::Custom;
        }
    });
}

fn render_target_and_split_panel(
    ui: &mut egui::Ui,
    state: &mut TruckLoadState,
    draft: &Problem,
    bundle: Option<&SolutionBundle>,
    plan: &TruckCapacityPlan,
    action: &mut Option<TruckLoadAction>,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong("3. Compartment Split, Target Load & Sync");
        ui.add_space(4.0);

        ui.horizontal_wrapped(|ui| {
            ui.label("Compartment split:");
            ui.selectable_value(
                &mut state.split_mode,
                CompartmentSplitMode::MatchTargetLoad,
                "Match Demand Ratio",
            );
            ui.selectable_value(
                &mut state.split_mode,
                CompartmentSplitMode::ManualPercentages {
                    ambient_pct: state.manual_ambient_pct,
                    chilled_pct: state.manual_chilled_pct,
                    frozen_pct: state.manual_frozen_pct,
                },
                "Manual %",
            );
            ui.selectable_value(
                &mut state.split_mode,
                CompartmentSplitMode::SingleZoneAmbient,
                "Single-Zone",
            );
        });

        if matches!(
            state.split_mode,
            CompartmentSplitMode::ManualPercentages { .. }
        ) {
            ui.horizontal_wrapped(|ui| {
                ui.label("Ambient %:");
                ui.add(
                    egui::DragValue::new(&mut state.manual_ambient_pct)
                        .range(0.0..=100.0)
                        .fixed_decimals(0),
                );
                ui.label("Chilled %:");
                ui.add(
                    egui::DragValue::new(&mut state.manual_chilled_pct)
                        .range(0.0..=100.0)
                        .fixed_decimals(0),
                );
                ui.label("Frozen %:");
                ui.add(
                    egui::DragValue::new(&mut state.manual_frozen_pct)
                        .range(0.0..=100.0)
                        .fixed_decimals(0),
                );
            });
        }

        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label("Load source:");
            for src in [
                LoadTargetSource::DraftVehicle,
                LoadTargetSource::DraftTotalDemand,
                LoadTargetSource::SolvedRoute,
                LoadTargetSource::CustomLoad,
            ] {
                ui.selectable_value(&mut state.target_source, src, src.label());
            }
        });

        match state.target_source {
            LoadTargetSource::DraftVehicle => {
                if !draft.vehicles.is_empty() {
                    let cur_name = &draft.vehicles[state.selected_vehicle_idx].name;
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Draft vehicle:");
                        egui::ComboBox::from_id_salt("truck_draft_veh_combo")
                            .selected_text(format!("V{}: {cur_name}", state.selected_vehicle_idx))
                            .show_ui(ui, |ui| {
                                for (idx, v) in draft.vehicles.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut state.selected_vehicle_idx,
                                        idx,
                                        format!(
                                            "V{idx}: {} (A:{} C:{} F:{})",
                                            v.name,
                                            v.capacity.ambient,
                                            v.capacity.chilled,
                                            v.capacity.frozen
                                        ),
                                    );
                                }
                            });
                    });
                    let selected_cap = draft.vehicles[state.selected_vehicle_idx].capacity;
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(format!(
                            "Configured: A:{} C:{} F:{} ({} total)  |  Recommended: A:{} C:{} F:{} ({} total)",
                            selected_cap.ambient,
                            selected_cap.chilled,
                            selected_cap.frozen,
                            selected_cap.total(),
                            plan.recommended_capacity.ambient,
                            plan.recommended_capacity.chilled,
                            plan.recommended_capacity.frozen,
                            plan.recommended_capacity.total(),
                        ));
                        if ui
                            .button("Copy Vehicle Cap -> Custom Load")
                            .on_hover_text("Copy the selected vehicle's configured A/C/F capacity into Custom Load for manual editing and bay visualization.")
                            .clicked()
                        {
                            state.custom_load = selected_cap;
                            state.target_source = LoadTargetSource::CustomLoad;
                        }
                    });
                } else {
                    ui.weak("No vehicles in the Import builder draft yet.");
                }
            }
            LoadTargetSource::SolvedRoute => {
                if let Some(b) = bundle
                    && !b.solution.routes.is_empty()
                {
                    let cur_r = state.selected_route_idx;
                    let vlabel = vehicle_name(&b.problem, &b.solution.routes[cur_r]);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Solved route:");
                        egui::ComboBox::from_id_salt("truck_solved_route_combo")
                            .selected_text(format!("Route {} ({vlabel})", cur_r + 1))
                            .show_ui(ui, |ui| {
                                for (idx, route) in b.solution.routes.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut state.selected_route_idx,
                                        idx,
                                        format!(
                                            "Route {} ({})",
                                            idx + 1,
                                            vehicle_name(&b.problem, route)
                                        ),
                                    );
                                }
                            });
                    });
                } else {
                    ui.weak("No solution loaded yet — load or solve a problem to inspect route LIFO stops.");
                }
            }
            LoadTargetSource::CustomLoad => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Custom A / C / F units:");
                    ui.add(egui::DragValue::new(&mut state.custom_load.ambient).range(0..=500));
                    ui.add(egui::DragValue::new(&mut state.custom_load.chilled).range(0..=500));
                    ui.add(egui::DragValue::new(&mut state.custom_load.frozen).range(0..=500));
                });
            }
            LoadTargetSource::DraftTotalDemand => {
                ui.weak(format!(
                    "Total draft problem demand: A:{} C:{} F:{} ({} total units across {} stores).",
                    plan.target_load.ambient,
                    plan.target_load.chilled,
                    plan.target_load.frozen,
                    plan.target_load.total(),
                    draft.stores.len()
                ));
            }
        }

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut state.preview_max_fill,
                "Fill Diagram to Max Capacity",
            );
            ui.checkbox(&mut state.show_stop_labels, "Show LIFO Stop Tags");
        });

        ui.separator();
        ui.horizontal_wrapped(|ui| {
            let can_apply_veh = !draft.vehicles.is_empty() && plan.effective_max_units > 0;
            if ui
                .add_enabled(
                    can_apply_veh,
                    egui::Button::new("Apply Capacity to Selected Vehicle"),
                )
                .clicked()
            {
                *action = Some(TruckLoadAction::SetDraftVehicleCapacity {
                    vehicle_idx: state.selected_vehicle_idx,
                    capacity: plan.recommended_capacity,
                });
            }
            if ui
                .add_enabled(
                    can_apply_veh,
                    egui::Button::new("Apply Capacity to All Vehicles"),
                )
                .clicked()
            {
                *action = Some(TruckLoadAction::SetAllDraftVehicleCapacities {
                    capacity: plan.recommended_capacity,
                });
            }
            if ui
                .button("Sync GVM & Height -> Solve Tab")
                .on_hover_text("Sets HGV mode, maxweight (tonnes) and maxheight (metres) in the Solve tab for OSM road legality routing.")
                .clicked()
            {
                *action = Some(TruckLoadAction::SyncSolverLimits {
                    max_weight_tonnes: state.truck.max_gvm_tonnes(),
                    max_height_m: state.truck.external_height_m,
                });
            }
        });
    });
}

fn render_diagnostics_panel(ui: &mut egui::Ui, plan: &TruckCapacityPlan) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong("Capacity Optimization Breakdown & Advice");
        ui.add_space(4.0);

        egui::Grid::new("truck_capacity_limits_grid")
            .striped(true)
            .num_columns(3)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.strong("Constraint");
                ui.strong("Max Units");
                ui.strong("Detail");
                ui.end_row();

                ui.label("Floor × Stack (Geometry)");
                ui.strong(plan.geometric_max_units.to_string());
                ui.label(format!(
                    "{} ({} / layer × {} layers)",
                    plan.orientation.label(),
                    plan.units_per_layer,
                    plan.stack_layers
                ));
                ui.end_row();

                ui.label("Usable Volume Limit");
                ui.strong(plan.volume_max_units.to_string());
                ui.label(format!(
                    "{:.2} m³ usable ({:.2} m³ gross − {} bulkhead gap)",
                    plan.usable_bay_volume_m3, plan.gross_bay_volume_m3, plan.active_bulkheads
                ));
                ui.end_row();

                ui.label("Payload Weight Limit");
                ui.strong(plan.weight_max_units.to_string());
                ui.label(format!("Active bottleneck: {}", plan.bottleneck.label()));
                ui.end_row();

                ui.label("Used Envelope (L×W×H)");
                ui.strong(format!(
                    "{:.2}×{:.2}×{:.2} m",
                    plan.used_length_m, plan.used_width_m, plan.used_height_m
                ));
                ui.label(format!(
                    "CoG {:.0}% from front · {} · Dead vol {:.2} m³",
                    plan.cog_from_front_pct,
                    plan.axle_balance.label(),
                    plan.dead_volume_m3
                ));
                ui.end_row();
            });

        if !plan.recommendations.is_empty() {
            ui.add_space(6.0);
            ui.strong("Optimization Notes");
            for tip in &plan.recommendations {
                ui.label(format!("• {tip}"));
            }
        }
    });
}

fn render_visual_panel(ui: &mut egui::Ui, state: &mut TruckLoadState, plan: &TruckCapacityPlan) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("Truck Loading Visualizer — {}", plan.target_label));
            ui.separator();
            for mode in TruckViewMode::ALL {
                ui.selectable_value(&mut state.view_mode, mode, mode.label());
            }
        });

        ui.horizontal_wrapped(|ui| {
            draw_legend_pill(ui, CompartmentKind::Frozen.color(), "Frozen (Front)");
            draw_legend_pill(ui, CompartmentKind::Chilled.color(), "Chilled (Mid)");
            draw_legend_pill(ui, CompartmentKind::Ambient.color(), "Ambient (Rear)");
            draw_legend_pill(ui, egui::Color32::from_rgb(100, 110, 125), "Available Slot");
            draw_legend_pill(
                ui,
                egui::Color32::from_rgb(185, 65, 65),
                "Weight/Vol Locked or Overflow",
            );
        });
        ui.add_space(4.0);

        match state.view_mode {
            TruckViewMode::AllViews => {
                draw_top_down_deck(ui, &state.truck, plan, state.show_stop_labels, 260.0);
                ui.add_space(6.0);
                ui.columns(2, |sub| {
                    draw_side_elevation(&mut sub[0], &state.truck, &state.unit, plan, 185.0);
                    draw_rear_cross_section(&mut sub[1], &state.truck, &state.unit, plan, 185.0);
                });
            }
            TruckViewMode::TopDownDeck => {
                draw_top_down_deck(ui, &state.truck, plan, state.show_stop_labels, 360.0);
            }
            TruckViewMode::SideElevation => {
                draw_side_elevation(ui, &state.truck, &state.unit, plan, 320.0);
            }
            TruckViewMode::RearCrossSection => {
                draw_rear_cross_section(ui, &state.truck, &state.unit, plan, 320.0);
            }
        }
    });
}

fn draw_legend_pill(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, color);
        ui.weak(label);
    });
}

fn draw_top_down_deck(
    ui: &mut egui::Ui,
    truck: &TruckDimensions,
    plan: &TruckCapacityPlan,
    show_stop_labels: bool,
    height_px: f32,
) {
    ui.weak(format!(
        "Top-Down Cargo Deck Plan ({:.2}m L × {:.2}m W · {}) — Front/Cab at Left, Rear Doors at Right",
        truck.bay_length_m,
        truck.bay_width_m,
        plan.orientation.label()
    ));

    let avail_w = ui.available_width().max(320.0);
    let (canvas, _) = ui.allocate_exact_size(egui::vec2(avail_w, height_px), egui::Sense::hover());
    let painter = ui.painter_at(canvas);

    painter.rect_filled(canvas, 6.0, egui::Color32::from_rgb(24, 28, 36));
    painter.rect_stroke(
        canvas,
        6.0,
        egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
        egui::StrokeKind::Inside,
    );

    let bay_l = truck.bay_length_m.max(0.1) as f32;
    let bay_w = truck.bay_width_m.max(0.1) as f32;

    let cab_reserved = 64.0_f32;
    let overflow_reserved = if plan.overflow_load.total() > 0 {
        82.0
    } else {
        36.0
    };
    let pad_top = 28.0_f32;
    let pad_bot = 26.0_f32;
    let max_bay_w_px = (avail_w - cab_reserved - overflow_reserved - 24.0).max(80.0);
    let max_bay_h_px = (height_px - pad_top - pad_bot).max(60.0);
    let scale = (max_bay_w_px / bay_l).min(max_bay_h_px / bay_w);

    let bay_px_w = bay_l * scale;
    let bay_px_h = bay_w * scale;
    let bay_x0 = canvas.left() + cab_reserved + ((max_bay_w_px - bay_px_w) * 0.5).max(0.0);
    let bay_y0 = canvas.top() + pad_top + ((max_bay_h_px - bay_px_h) * 0.5).max(0.0);
    let bay_rect =
        egui::Rect::from_min_size(egui::pos2(bay_x0, bay_y0), egui::vec2(bay_px_w, bay_px_h));

    // Stylized truck cab on the left side.
    let cab_w = 46.0_f32.min(bay_x0 - canvas.left() - 8.0).max(26.0);
    let cab_h = (bay_px_h * 0.84).max(36.0);
    let cab_rect = egui::Rect::from_min_size(
        egui::pos2(
            bay_rect.left() - cab_w - 4.0,
            bay_rect.center().y - cab_h * 0.5,
        ),
        egui::vec2(cab_w, cab_h),
    );
    painter.rect_filled(cab_rect, 6.0, egui::Color32::from_rgb(56, 66, 82));
    painter.rect_stroke(
        cab_rect,
        6.0,
        egui::Stroke::new(1.5, egui::Color32::from_rgb(130, 145, 165)),
        egui::StrokeKind::Inside,
    );
    let windshield = egui::Rect::from_min_size(
        egui::pos2(cab_rect.left() + 5.0, cab_rect.top() + 8.0),
        egui::vec2(10.0, cab_h - 16.0),
    );
    painter.rect_filled(windshield, 3.0, egui::Color32::from_rgb(115, 170, 210));
    painter.text(
        egui::pos2(cab_rect.center().x + 4.0, cab_rect.center().y),
        egui::Align2::CENTER_CENTER,
        "CAB",
        egui::FontId::proportional(10.0),
        egui::Color32::from_gray(220),
    );

    // Cargo bay interior background & outer frame.
    painter.rect_filled(bay_rect, 3.0, egui::Color32::from_rgb(34, 40, 50));
    painter.rect_stroke(
        bay_rect,
        3.0,
        egui::Stroke::new(2.0, egui::Color32::from_rgb(175, 185, 200)),
        egui::StrokeKind::Outside,
    );

    // Rear swing doors indicator.
    let door_len = 16.0_f32;
    painter.line_segment(
        [
            bay_rect.right_top(),
            bay_rect.right_top() + egui::vec2(door_len, -door_len * 0.45),
        ],
        egui::Stroke::new(2.5, egui::Color32::from_rgb(190, 200, 215)),
    );
    painter.line_segment(
        [
            bay_rect.right_bottom(),
            bay_rect.right_bottom() + egui::vec2(door_len, door_len * 0.45),
        ],
        egui::Stroke::new(2.5, egui::Color32::from_rgb(190, 200, 215)),
    );

    // Draw zone shading & compartment labels along the top.
    for span in &plan.zone_spans {
        let sx0 = bay_rect.left() + span.x_start_m as f32 * scale;
        let sx1 = bay_rect.left() + span.x_end_m as f32 * scale;
        let zone_rect = egui::Rect::from_min_max(
            egui::pos2(sx0, bay_rect.top()),
            egui::pos2(sx1, bay_rect.bottom()),
        );
        let bg = span.compartment.color().linear_multiply(0.10);
        painter.rect_filled(zone_rect, 0.0, bg);
        painter.text(
            egui::pos2((sx0 + sx1) * 0.5, bay_rect.top() - 10.0),
            egui::Align2::CENTER_CENTER,
            format!(
                "{} {}/{}",
                span.compartment.short_code(),
                span.loaded_units,
                span.capacity_units
            ),
            egui::FontId::proportional(10.5),
            span.compartment.color(),
        );
    }

    // Draw insulated bulkheads.
    for &bh_x in &plan.bulkhead_positions_m {
        let x0 = bay_rect.left() + bh_x as f32 * scale;
        let bh_w = (truck.bulkhead_thickness_m as f32 * scale).max(2.5);
        let bh_rect =
            egui::Rect::from_min_size(egui::pos2(x0, bay_rect.top()), egui::vec2(bh_w, bay_px_h));
        painter.rect_filled(bh_rect, 0.0, egui::Color32::from_rgb(200, 212, 226));
    }

    // Draw each handling-unit slot and register hover interaction.
    let pointer_pos = ui.input(|i| i.pointer.hover_pos());
    let mut hovered_tip: Option<&str> = None;

    for slot in &plan.slots {
        let r = egui::Rect::from_min_size(
            egui::pos2(
                bay_rect.left() + slot.x_start_m as f32 * scale + 1.2,
                bay_rect.top() + slot.y_start_m as f32 * scale + 1.2,
            ),
            egui::vec2(
                (slot.slot_length_m as f32 * scale - 2.4).max(4.0),
                (slot.slot_width_m as f32 * scale - 2.4).max(4.0),
            ),
        );

        match slot.status {
            LoadedSlotStatus::Loaded => {
                let fill = slot.compartment.color().linear_multiply(0.88);
                painter.rect_filled(r, 2.5, fill);
                painter.rect_stroke(
                    r,
                    2.5,
                    egui::Stroke::new(1.2, egui::Color32::WHITE.linear_multiply(0.8)),
                    egui::StrokeKind::Inside,
                );
                if r.width() >= 18.0 && r.height() >= 14.0 {
                    let mut text = String::new();
                    if slot.max_layers > 1 {
                        text.push_str(&format!("×{}", slot.loaded_layers));
                    } else {
                        text.push_str(slot.compartment.short_code());
                    }
                    if show_stop_labels && !slot.stop_badges.is_empty() && r.height() >= 22.0 {
                        text.push('\n');
                        text.push_str(&slot.stop_badges.join(","));
                    }
                    painter.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional((r.height() * 0.36).clamp(8.5, 11.5)),
                        egui::Color32::from_rgb(15, 20, 26),
                    );
                }
            }
            LoadedSlotStatus::EmptyCapacity => {
                painter.rect_filled(r, 2.5, slot.compartment.color().linear_multiply(0.18));
                painter.rect_stroke(
                    r,
                    2.5,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(115, 130, 148)),
                    egui::StrokeKind::Inside,
                );
            }
            LoadedSlotStatus::WeightOrVolumeLocked => {
                painter.rect_filled(
                    r,
                    2.5,
                    egui::Color32::from_rgba_unmultiplied(150, 50, 50, 65),
                );
                painter.rect_stroke(
                    r,
                    2.5,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(200, 80, 80)),
                    egui::StrokeKind::Inside,
                );
                painter.line_segment(
                    [r.left_top(), r.right_bottom()],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(200, 80, 80)),
                );
            }
        }

        if let Some(pos) = pointer_pos
            && r.contains(pos)
        {
            hovered_tip = Some(&slot.tooltip);
        }
    }

    // Bottom length / width dimension readout.
    painter.text(
        egui::pos2(bay_rect.center().x, bay_rect.bottom() + 12.0),
        egui::Align2::CENTER_CENTER,
        format!(
            "Internal Bay: {:.2}m L × {:.2}m W · Used Deck: {:.2}m × {:.2}m",
            truck.bay_length_m, truck.bay_width_m, plan.used_length_m, plan.used_width_m
        ),
        egui::FontId::proportional(10.5),
        egui::Color32::from_gray(195),
    );

    if plan.overflow_load.total() > 0 {
        let ov_rect = egui::Rect::from_min_size(
            egui::pos2(bay_rect.right() + 14.0, bay_rect.center().y - 30.0),
            egui::vec2(66.0, 60.0),
        );
        painter.rect_filled(ov_rect, 4.0, egui::Color32::from_rgb(130, 35, 35));
        painter.rect_stroke(
            ov_rect,
            4.0,
            egui::Stroke::new(1.2, egui::Color32::from_rgb(240, 100, 100)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            ov_rect.center(),
            egui::Align2::CENTER_CENTER,
            format!(
                "OVERFLOW\n+{}\nA:{} C:{} F:{}",
                plan.overflow_load.total(),
                plan.overflow_load.ambient,
                plan.overflow_load.chilled,
                plan.overflow_load.frozen
            ),
            egui::FontId::proportional(9.5),
            egui::Color32::WHITE,
        );
    }

    if let Some(tip) = hovered_tip {
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            ui.scope_id().with("truck_slot_hover_tip"),
            egui::PopupAnchor::Pointer,
        )
        .show(|ui| {
            ui.label(tip);
        });
    }
}

fn draw_side_elevation(
    ui: &mut egui::Ui,
    truck: &TruckDimensions,
    unit: &HandlingUnitSpec,
    plan: &TruckCapacityPlan,
    height_px: f32,
) {
    ui.weak(format!(
        "Side Profile ({:.2}m L × {:.2}m H · {} stack layer(s))",
        truck.bay_length_m, truck.bay_height_m, plan.stack_layers
    ));

    let avail_w = ui.available_width().max(220.0);
    let (canvas, _) = ui.allocate_exact_size(egui::vec2(avail_w, height_px), egui::Sense::hover());
    let painter = ui.painter_at(canvas);

    painter.rect_filled(canvas, 6.0, egui::Color32::from_rgb(24, 28, 36));
    painter.rect_stroke(
        canvas,
        6.0,
        egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
        egui::StrokeKind::Inside,
    );

    let bay_l = truck.bay_length_m.max(0.1) as f32;
    let bay_h = truck.bay_height_m.max(0.1) as f32;

    let cab_w = 40.0_f32;
    let ground_pad = 26.0_f32;
    let top_pad = 20.0_f32;
    let max_w = (avail_w - cab_w - 28.0).max(60.0);
    let max_h = (height_px - ground_pad - top_pad).max(40.0);
    let scale = (max_w / bay_l).min(max_h / bay_h);

    let body_w = bay_l * scale;
    let body_h = bay_h * scale;
    let body_x0 = canvas.left() + cab_w + 10.0 + ((max_w - body_w) * 0.5).max(0.0);
    let body_y0 = canvas.bottom() - ground_pad - body_h;
    let body_rect =
        egui::Rect::from_min_size(egui::pos2(body_x0, body_y0), egui::vec2(body_w, body_h));

    // Road line + wheels.
    let road_y = body_rect.bottom() + 14.0;
    painter.line_segment(
        [
            egui::pos2(canvas.left() + 8.0, road_y),
            egui::pos2(canvas.right() - 8.0, road_y),
        ],
        egui::Stroke::new(1.5, egui::Color32::from_gray(90)),
    );
    let cab_rect = egui::Rect::from_min_size(
        egui::pos2(
            body_rect.left() - cab_w + 2.0,
            body_rect.bottom() - (body_h * 0.72).max(34.0),
        ),
        egui::vec2(cab_w - 4.0, (body_h * 0.72).max(34.0)),
    );
    painter.rect_filled(cab_rect, 4.0, egui::Color32::from_rgb(56, 66, 82));
    for wx in [
        cab_rect.center().x,
        body_rect.right() - (body_w * 0.22).max(16.0),
    ] {
        let wheel_c = egui::pos2(wx, road_y - 7.0);
        painter.circle_filled(wheel_c, 7.5, egui::Color32::from_gray(35));
        painter.circle_stroke(
            wheel_c,
            7.5,
            egui::Stroke::new(1.8, egui::Color32::from_gray(170)),
        );
    }

    // Cargo body outline.
    painter.rect_filled(body_rect, 2.0, egui::Color32::from_rgb(34, 40, 50));
    painter.rect_stroke(
        body_rect,
        2.0,
        egui::Stroke::new(1.5, egui::Color32::from_rgb(175, 185, 200)),
        egui::StrokeKind::Outside,
    );

    // Door clearance dashed line when smaller than internal bay height.
    if truck.door_clearance_m > 0.0 && truck.door_clearance_m < truck.bay_height_m {
        let door_y = body_rect.bottom() - truck.door_clearance_m as f32 * scale;
        painter.line_segment(
            [
                egui::pos2(body_rect.right() - 22.0, door_y),
                egui::pos2(body_rect.right(), door_y),
            ],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(245, 166, 35)),
        );
    }

    // Group loaded/available heights per distinct longitudinal row.
    let unit_h_px = unit.height_m.max(0.05) as f32 * scale;
    let mut rows_drawn: BTreeMap<u32, (f64, f64, u32, CompartmentKind, bool)> = BTreeMap::new();
    for slot in &plan.slots {
        let entry = rows_drawn.entry(slot.row).or_insert((
            slot.x_start_m,
            slot.slot_length_m,
            0,
            slot.compartment,
            false,
        ));
        entry.2 = entry.2.max(slot.loaded_layers);
        if slot.status == LoadedSlotStatus::Loaded {
            entry.4 = true;
        }
    }

    for (x_m, len_m, max_l, comp, is_loaded) in rows_drawn.values() {
        let x0 = body_rect.left() + *x_m as f32 * scale + 1.0;
        let w = (*len_m as f32 * scale - 2.0).max(3.0);
        let draw_layers = if *is_loaded {
            *max_l
        } else {
            plan.stack_layers
        };
        for layer in 0..draw_layers {
            let y1 = body_rect.bottom() - layer as f32 * unit_h_px - 1.0;
            let y0 = (y1 - unit_h_px + 2.0).max(body_rect.top());
            let cell = egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x0 + w, y1));
            if *is_loaded {
                painter.rect_filled(cell, 1.5, comp.color().linear_multiply(0.85));
                painter.rect_stroke(
                    cell,
                    1.5,
                    egui::Stroke::new(0.8, egui::Color32::WHITE.linear_multiply(0.7)),
                    egui::StrokeKind::Inside,
                );
            } else {
                painter.rect_stroke(
                    cell,
                    1.5,
                    egui::Stroke::new(0.7, egui::Color32::from_gray(85)),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }

    // Bulkheads in side view.
    for &bh_x in &plan.bulkhead_positions_m {
        let x0 = body_rect.left() + bh_x as f32 * scale;
        painter.line_segment(
            [
                egui::pos2(x0, body_rect.top()),
                egui::pos2(x0, body_rect.bottom()),
            ],
            egui::Stroke::new(2.5, egui::Color32::from_rgb(200, 212, 226)),
        );
    }

    // Center-of-Gravity marker when loaded.
    if plan.axle_balance != AxleBalanceStatus::Unloaded {
        let cog_x = body_rect.left() + body_w * (plan.cog_from_front_pct as f32 / 100.0);
        painter.circle_filled(
            egui::pos2(cog_x, body_rect.bottom()),
            4.0,
            egui::Color32::from_rgb(255, 220, 80),
        );
    }

    painter.text(
        egui::pos2(canvas.center().x, canvas.top() + 10.0),
        egui::Align2::CENTER_CENTER,
        format!(
            "Headspace: {:.2}m · CoG: {:.0}% ({})",
            (truck.bay_height_m - plan.used_height_m).max(0.0),
            plan.cog_from_front_pct,
            plan.axle_balance.label()
        ),
        egui::FontId::proportional(10.0),
        egui::Color32::from_gray(200),
    );
}

fn draw_rear_cross_section(
    ui: &mut egui::Ui,
    truck: &TruckDimensions,
    unit: &HandlingUnitSpec,
    plan: &TruckCapacityPlan,
    height_px: f32,
) {
    ui.weak(format!(
        "Rear Cross-Section ({:.2}m W × {:.2}m H · Door {:.2}m)",
        truck.bay_width_m, truck.bay_height_m, plan.effective_ceiling_m
    ));

    let avail_w = ui.available_width().max(180.0);
    let (canvas, _) = ui.allocate_exact_size(egui::vec2(avail_w, height_px), egui::Sense::hover());
    let painter = ui.painter_at(canvas);

    painter.rect_filled(canvas, 6.0, egui::Color32::from_rgb(24, 28, 36));
    painter.rect_stroke(
        canvas,
        6.0,
        egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
        egui::StrokeKind::Inside,
    );

    let bay_w = truck.bay_width_m.max(0.1) as f32;
    let bay_h = truck.bay_height_m.max(0.1) as f32;
    let scale = ((avail_w - 36.0) / bay_w).min((height_px - 44.0) / bay_h);

    let rect_w = bay_w * scale;
    let rect_h = bay_h * scale;
    let x0 = canvas.center().x - rect_w * 0.5;
    let y0 = canvas.top() + 20.0 + ((height_px - 44.0 - rect_h) * 0.5).max(0.0);
    let frame_rect = egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(rect_w, rect_h));

    painter.rect_filled(frame_rect, 2.0, egui::Color32::from_rgb(34, 40, 50));
    painter.rect_stroke(
        frame_rect,
        2.0,
        egui::Stroke::new(2.0, egui::Color32::from_rgb(175, 185, 200)),
        egui::StrokeKind::Outside,
    );

    // Door header limit line across the top of the rear opening.
    if truck.door_clearance_m > 0.0 && truck.door_clearance_m < truck.bay_height_m {
        let dy = frame_rect.bottom() - truck.door_clearance_m as f32 * scale;
        painter.line_segment(
            [
                egui::pos2(frame_rect.left(), dy),
                egui::pos2(frame_rect.right(), dy),
            ],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(245, 166, 35)),
        );
    }

    // Show the rear-most row cross section (closest to doors) if slots exist.
    let rear_row = plan.slots.iter().map(|s| s.row).max().unwrap_or(0);
    let rear_slots: Vec<&FloorSlotPlan> = plan.slots.iter().filter(|s| s.row == rear_row).collect();
    let unit_h_px = unit.height_m.max(0.05) as f32 * scale;

    for slot in rear_slots {
        let sx0 = frame_rect.left() + slot.y_start_m as f32 * scale + 1.5;
        let sw = (slot.slot_width_m as f32 * scale - 3.0).max(4.0);
        let draw_layers = if slot.status == LoadedSlotStatus::Loaded {
            slot.loaded_layers
        } else {
            plan.stack_layers
        };
        for layer in 0..draw_layers {
            let sy1 = frame_rect.bottom() - layer as f32 * unit_h_px - 1.0;
            let sy0 = (sy1 - unit_h_px + 2.0).max(frame_rect.top());
            let cell = egui::Rect::from_min_max(egui::pos2(sx0, sy0), egui::pos2(sx0 + sw, sy1));
            if slot.status == LoadedSlotStatus::Loaded {
                painter.rect_filled(cell, 2.0, slot.compartment.color().linear_multiply(0.88));
                painter.rect_stroke(
                    cell,
                    2.0,
                    egui::Stroke::new(1.0, egui::Color32::WHITE.linear_multiply(0.75)),
                    egui::StrokeKind::Inside,
                );
            } else {
                painter.rect_stroke(
                    cell,
                    2.0,
                    egui::Stroke::new(0.9, egui::Color32::from_gray(95)),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }

    let side_gap = (truck.bay_width_m - plan.used_width_m).max(0.0);
    let top_gap = (plan.effective_ceiling_m - plan.used_height_m).max(0.0);
    painter.text(
        egui::pos2(canvas.center().x, canvas.bottom() - 10.0),
        egui::Align2::CENTER_CENTER,
        format!(
            "Cols: {} · Side Gap: {:.2}m · Door/Roof Gap: {:.2}m",
            plan.max_cols_across, side_gap, top_gap
        ),
        egui::FontId::proportional(10.0),
        egui::Color32::from_gray(195),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_target() -> TargetResolution {
        TargetResolution {
            source_label: "Test".to_string(),
            target_load: Load::ZERO,
            total_problem_demand: Load::ZERO,
            stops: Vec::new(),
        }
    }

    #[test]
    fn rotation_improves_floor_count_when_dimension_aligns_better() {
        // Bay: 3.60m × 2.40m × 2.40m, single zone (0 bulkheads)
        // Unit: 1.20m × 0.80m × 1.20m, stack=1
        // Lengthwise (1.20 along L, 0.80 along W): floor(3.60/1.20)*floor(2.40/0.80) = 3 * 3 = 9
        // Compare against Bay 2.40m × 3.60m? Let's test bay 2.50m × 2.45m with unit 1.20×0.80:
        // Standard: rows = floor(2.50/1.20)=2, cols = floor(2.45/0.80)=3 -> 6
        // Rotated: rows = floor(2.50/0.80)=3, cols = floor(2.45/1.20)=2 -> 6
        // Now try bay 2.60m × 2.40m with unit 1.30m × 0.80m? Wait:
        // Try bay 3.25m × 2.00m with unit 1.00m × 0.80m:
        // Standard: rows = floor(3.25/1.00)=3, cols = floor(2.00/0.80)=2 -> 6
        // Rotated: rows = floor(3.25/0.80)=4, cols = floor(2.00/1.00)=2 -> 8 (+2!)
        let truck = TruckDimensions {
            bay_length_m: 3.25,
            bay_width_m: 2.00,
            bay_height_m: 2.00,
            door_clearance_m: 2.00,
            bulkhead_thickness_m: 0.0,
            use_volume_override: false,
            volume_override_m3: 13.0,
            usable_volume_efficiency_pct: 100.0,
            max_payload_kg: 10_000.0,
            tare_weight_kg: 3_000.0,
            external_height_m: 3.0,
        };
        let mut unit = UnitPreset::AuRollCage.spec();
        unit.length_m = 1.00;
        unit.width_m = 0.80;
        unit.height_m = 1.50;
        unit.max_stack_layers = 1;
        unit.allow_rotation = true;

        let plan = compute_truck_capacity_plan(
            &truck,
            &unit,
            CompartmentSplitMode::SingleZoneAmbient,
            &zero_target(),
            true,
        );
        assert_eq!(plan.units_per_layer, 8);
        assert_eq!(plan.rotation_gain_units, 2);
        assert!(matches!(
            plan.orientation,
            FloorOrientationChoice::Rotated90 { rows: 4, cols: 2 }
        ));
    }

    #[test]
    fn mixed_row_packing_beats_pure_orientations() {
        // Bay: 2.80m × 2.40m, Unit: 1.20m × 0.80m
        // Pure Standard (1.20 × 0.80): floor(2.80/1.20)=2 rows × floor(2.40/0.80)=3 cols = 6 units
        // Pure Rotated (0.80 × 1.20): floor(2.80/0.80)=3 rows × floor(2.40/1.20)=2 cols = 6 units
        // Mixed: 1 std row (1.20m) = 3 units, remaining 1.60m = 2 rot rows × 2 cols = 4 units => 7 units!
        let truck = TruckDimensions {
            bay_length_m: 2.80,
            bay_width_m: 2.40,
            bay_height_m: 2.40,
            door_clearance_m: 2.40,
            bulkhead_thickness_m: 0.0,
            use_volume_override: false,
            volume_override_m3: 16.0,
            usable_volume_efficiency_pct: 100.0,
            max_payload_kg: 20_000.0,
            tare_weight_kg: 5_000.0,
            external_height_m: 3.4,
        };
        let mut unit = UnitPreset::EuroPallet.spec();
        unit.max_stack_layers = 1;
        unit.allow_rotation = true;

        let plan = compute_truck_capacity_plan(
            &truck,
            &unit,
            CompartmentSplitMode::SingleZoneAmbient,
            &zero_target(),
            true,
        );
        assert_eq!(plan.units_per_layer, 7);
        assert!(matches!(
            plan.orientation,
            FloorOrientationChoice::Mixed {
                std_rows: 1,
                std_cols: 3,
                rot_rows: 2,
                rot_cols: 2,
            }
        ));
    }

    #[test]
    fn weight_limit_creates_payload_bottleneck_and_locked_slots() {
        let truck = TruckDimensions {
            bay_length_m: 6.60,
            bay_width_m: 2.40,
            bay_height_m: 2.40,
            door_clearance_m: 2.40,
            bulkhead_thickness_m: 0.0,
            use_volume_override: false,
            volume_override_m3: 38.0,
            usable_volume_efficiency_pct: 100.0,
            max_payload_kg: 2_000.0, // Fits only 4 units at 500 kg each
            tare_weight_kg: 4_000.0,
            external_height_m: 3.4,
        };
        let mut unit = UnitPreset::AuRollCage.spec();
        unit.ambient_weight_kg = 500.0;
        unit.max_stack_layers = 1;

        let plan = compute_truck_capacity_plan(
            &truck,
            &unit,
            CompartmentSplitMode::SingleZoneAmbient,
            &zero_target(),
            true,
        );
        assert_eq!(plan.weight_max_units, 4);
        assert_eq!(plan.effective_max_units, 4);
        assert_eq!(plan.bottleneck, BottleneckKind::PayloadWeightLimit);
        assert!(
            plan.slots
                .iter()
                .any(|s| s.status == LoadedSlotStatus::WeightOrVolumeLocked)
        );
    }

    #[test]
    fn door_clearance_bottleneck_detected_when_lower_than_roof() {
        let truck = TruckDimensions {
            bay_length_m: 4.80,
            bay_width_m: 2.40,
            bay_height_m: 2.90,     // tall enough for 2 layers of 1.40m
            door_clearance_m: 2.60, // blocks layer 2 (needs 2.80m)
            bulkhead_thickness_m: 0.0,
            use_volume_override: false,
            volume_override_m3: 30.0,
            usable_volume_efficiency_pct: 100.0,
            max_payload_kg: 30_000.0,
            tare_weight_kg: 5_000.0,
            external_height_m: 4.0,
        };
        let unit = UnitPreset::AuStandardPallet.spec(); // height 1.40, max_stack 2

        let plan = compute_truck_capacity_plan(
            &truck,
            &unit,
            CompartmentSplitMode::SingleZoneAmbient,
            &zero_target(),
            true,
        );
        assert_eq!(plan.stack_layers, 1);
        assert_eq!(plan.bottleneck, BottleneckKind::DoorClearance);
    }

    #[test]
    fn lifo_stop_ordering_puts_last_delivery_deepest() {
        let truck = TruckDimensions {
            bay_length_m: 4.00,
            bay_width_m: 2.40,
            bay_height_m: 2.20,
            door_clearance_m: 2.20,
            bulkhead_thickness_m: 0.0,
            use_volume_override: false,
            volume_override_m3: 20.0,
            usable_volume_efficiency_pct: 100.0,
            max_payload_kg: 15_000.0,
            tare_weight_kg: 4_000.0,
            external_height_m: 3.3,
        };
        let unit = UnitPreset::AuRollCage.spec();
        let target = TargetResolution {
            source_label: "Route 1".to_string(),
            target_load: Load::new(2, 0, 0),
            total_problem_demand: Load::new(2, 0, 0),
            stops: vec![
                StopLoadTag {
                    sequence_no: 1,
                    store_name: "First Store".to_string(),
                    demand: Load::new(1, 0, 0),
                },
                StopLoadTag {
                    sequence_no: 2,
                    store_name: "Second Store".to_string(),
                    demand: Load::new(1, 0, 0),
                },
            ],
        };

        let plan = compute_truck_capacity_plan(
            &truck,
            &unit,
            CompartmentSplitMode::SingleZoneAmbient,
            &target,
            false,
        );
        let loaded_badges: Vec<&str> = plan
            .slots
            .iter()
            .filter(|s| s.status == LoadedSlotStatus::Loaded)
            .flat_map(|s| s.stop_badges.iter().map(String::as_str))
            .collect();
        // S2 (delivered second) is loaded first at front (deepest), S1 is loaded after (closer to rear door).
        assert_eq!(loaded_badges, vec!["S2", "S1"]);
    }

    #[test]
    fn zero_dimensions_handled_gracefully_without_panics() {
        let truck = TruckDimensions {
            bay_length_m: 0.0,
            bay_width_m: 0.0,
            bay_height_m: 0.0,
            door_clearance_m: 0.0,
            bulkhead_thickness_m: 0.0,
            use_volume_override: false,
            volume_override_m3: 0.0,
            usable_volume_efficiency_pct: 100.0,
            max_payload_kg: 0.0,
            tare_weight_kg: 0.0,
            external_height_m: 0.0,
        };
        let unit = UnitPreset::AuRollCage.spec();
        let plan = compute_truck_capacity_plan(
            &truck,
            &unit,
            CompartmentSplitMode::MatchTargetLoad,
            &zero_target(),
            false,
        );
        assert_eq!(plan.effective_max_units, 0);
        assert_eq!(plan.bottleneck, BottleneckKind::Unconstrained);
        assert!(plan.slots.is_empty());
    }
}
