//! kimchi-gen: a single harness over local and cloud image/video models.
//!
//! * [`Provider`] is the trait each backend implements.
//! * [`Harness`] owns providers, keys, settings and the job queue.

pub mod harness;
pub mod provider;
pub mod providers;
pub(crate) mod sound;
pub mod types;
pub mod util;
pub mod voices;

pub use harness::{Harness, Job, JobStatus, KeySource, MemorySecrets, ProviderSettings, ProviderStatus, SecretStore};
pub use provider::{Ctx, GenError, GenResult, Provider};
pub use types::*;
