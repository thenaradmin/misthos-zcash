//! Core payroll engine: employees, pay schedules, pay runs and per-run caps.
//!
//! This crate has no chain I/O. A finalized [`run::PayRun`] is the input to the
//! (later) ZIP-321 / zcash_client_backend proposal step.

pub mod config;
pub mod csv;
pub mod date;
pub mod employee;
pub mod engine;
pub mod money;
pub mod schedule;

pub use engine::{CapPolicy, Engine, EngineError, PayLine, PayRun, RunStatus};
