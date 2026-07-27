// Both Gateway flavors intentionally share one entrypoint implementation. A
// separate source path avoids Cargo's duplicate-target warning while the
// package feature selects the Pingora dependency family.
include!("edgion_gateway.rs");
