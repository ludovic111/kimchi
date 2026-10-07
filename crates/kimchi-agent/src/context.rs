//! What the person is looking at, sent with each request and refreshed before every model step.
//!
//! The summary itself is `kimchi_control::harness::context` (also the `harness.context` command,
//! which outside agents read): the built-in agent puts it in front of the request as a
//! `<context>` block, and before each later step adds an updated block to the tool results when
//! it changed, so the thread is only ever appended to (prompt caches and thinking blocks stay
//! valid).

pub use kimchi_control::harness::context::{Glance, glance, glance_since, unframed};
