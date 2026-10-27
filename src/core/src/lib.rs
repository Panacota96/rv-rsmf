//! RV RSMF core library.
//!
//! The core does all the work and never prints: front-ends (CLI, GUI) call [`run::run`] and
//! render the [`events::Event`] stream it emits.
//!
//! ```text
//! input ──► hash (1–2 algorithms) ──► case registry check ──► exports ──► parse ──► RSMF per conversation/day
//!                                                                     ──► validate + read back ──► trace ──► reports
//!   └──────────────────────── every step appended to the hash-chained audit log ────────────────────────┘
//! ```

pub mod analyzer;
pub mod audit;
pub mod case;
pub mod convert;
pub mod env;
pub mod events;
pub mod hash;
pub mod i18n;
pub mod input;
pub mod intake;
pub mod model;
pub mod report;
pub mod rsmf;
pub mod run;
pub mod sources;
pub mod time;
pub mod trust;
pub mod verify;

pub use case::CaseRole;
pub use events::Event;
pub use hash::HashAlgo;
pub use run::{RunConfig, RunSummary, run};

pub const TOOL_NAME: &str = "rv-rsmf";
/// Product version = version.txt (bumped by release-please). The crate versions in Cargo.toml stay fixed so a
/// release never changes Cargo.lock (CI builds with --locked).
pub const TOOL_VERSION: &str = env!("RV_VERSION");

/// This tool's place in a shared case folder (see docs/case-folder.md): rv-rsmf is an ANALYSIS tool — it verifies
/// every item against the registry written by the intake tool, rv-hashinv. Keep equal to `case_role` in .forge.json.
pub const CASE_ROLE: CaseRole = CaseRole::Analysis;
