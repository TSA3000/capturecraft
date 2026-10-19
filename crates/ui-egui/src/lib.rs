//! The CaptureCraft egui shell (L7). Only this layer may use egui, eframe
//! or winit; it draws [`state`] and turns every user action into an engine
//! command.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod state;
