//! S2 Lite v1 protocol and Android durable integration.
//! I6.4 exposes bounded migration/cutover authority and permanent legacy PUT
//! admission. Ordinary writer execution and mobile scheduling remain deferred.
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
pub mod webdav_adapter;

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

pub mod discovery_persistence;
#[cfg(test)]
mod discovery_persistence_tests;
pub mod discovery_runtime;
pub mod materialized_projection;

pub mod business_projection;
pub mod durable_persistence;
pub mod migration_admission;
pub mod root_coordinator;
pub mod target_root_binding;

pub mod migration_runtime;

#[cfg(test)]
mod migration_runtime_tests;
