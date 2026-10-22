//! Core building blocks shared by every April Blink Engine crate: math types, frame timing,
//! identifiers, logging setup, the worker-thread job system and small dependency-free
//! utilities.

pub mod color;
pub mod jobs;
pub mod logging;
pub mod math;
pub mod time;
pub mod types;
pub mod utils;

pub use color::Color;
pub use logging::{init_logging, log_file_path, log_to_directory};
pub use math::*;
pub use time::{FixedTimestep, Time};
pub use types::ResourceId;
