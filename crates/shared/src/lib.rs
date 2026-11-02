//! shared — common types, PortReport, errors and configuration for every phase crate.

pub mod catalog;
pub mod config;
pub mod error;
pub mod report;
pub mod types;
pub mod util;

pub use catalog::HookCatalog;
pub use config::PorterConfig;
pub use error::{ErrorType, Phase, PortError};
pub use report::{Overall, PortReport, VerificationReport};
pub use types::*;
