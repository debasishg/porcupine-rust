/// porcupine-rust: a fast linearizability checker.
///
/// Port of <https://github.com/anishathalye/porcupine>.
pub mod checker;
pub mod model;
pub mod types;

pub use model::Model;
pub use types::{CheckResult, Event, EventKind, LinearizationInfo, Operation};
