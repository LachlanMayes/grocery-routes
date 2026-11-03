//! Identifier newtypes. Each id indexes into the corresponding `Vec` on
//! [`crate::Problem`] (the M0 convention).

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(
    /// Identifies a depot; indexes `Problem::depots`.
    DepotId
);
id_type!(
    /// Identifies a store; indexes `Problem::stores`.
    StoreId
);
id_type!(
    /// Identifies a vehicle; indexes `Problem::vehicles`.
    VehicleId
);
id_type!(
    /// Identifies a driver; indexes `Problem::drivers`.
    DriverId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_display_as_numbers() {
        assert_eq!(StoreId(7).to_string(), "7");
        assert_eq!(VehicleId(12).to_string(), "12");
    }
}
