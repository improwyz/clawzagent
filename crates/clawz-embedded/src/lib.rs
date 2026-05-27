//! ClawZ Embedded Runtime — T0 bare-metal support (ESP32/Arduino)
//!
//! This crate provides the no_std embedded agent runtime using embassy-executor
//! for interrupt-driven async on ESP32-S3 and similar MCUs.

#![cfg_attr(feature = "no_std", no_std)]
#![cfg_attr(feature = "no_std", feature(type_alias_impl_trait))]

extern crate alloc;

pub mod embassy_backend;

pub use embassy_backend::EmbassyBackend;
