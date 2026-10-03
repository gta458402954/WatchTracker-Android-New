//! Frozen S2 Lite v1 pure protocol core.
//!
//! This Android bootstrap intentionally exposes no production coordinator,
//! persistence, or WebDAV transport surface.  S1 remains the only live sync
//! path until a later integration phase explicitly wires S2.
pub mod activation_cutover;
pub mod bootstrap;
pub mod canonical;
pub mod causal;
pub mod conflict;
pub mod immutable_publish;
pub mod local_authority;
pub(crate) mod local_capture;
#[cfg(test)]
mod local_capture_tests;
pub mod migration_orchestration;
pub mod ordinary_mutation;
pub mod remote_discovery;
pub mod semantic;
pub mod types;

#[cfg(test)]
mod activation_cutover_tests;
#[cfg(test)]
mod causal_tests;
#[cfg(test)]
mod fixture_manifest_tests;
#[cfg(test)]
mod immutable_publish_tests;
#[cfg(test)]
mod migration_orchestration_tests;
#[cfg(test)]
mod ordinary_mutation_tests;
#[cfg(test)]
mod remote_discovery_tests;
#[cfg(test)]
mod tests;
