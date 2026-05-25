//! Reality dimension — modelling the operational environment for PRISM-G.
//!
//! This module produces [`ContextBundle`] snapshots, detects drift between
//! predicted and observed reality, and versions bundles over time with tenant
//! isolation.
//!
//! | Sub-module | Responsibility |
//! |------------|----------------|
//! | [`discovery`] | Build a `ContextBundle` from mixed discovery sources |
//! | [`drift`]     | Compare predicted vs observed bundles, emit `DriftSignal`s |
//! | [`versioning`]| Store, retrieve, and reconstruct versioned bundles |

pub mod discovery;
pub mod drift;
pub mod versioning;

pub use discovery::RealityModel;
pub use drift::DriftDetector;
pub use versioning::{ContextStore, InMemoryContextStore};
