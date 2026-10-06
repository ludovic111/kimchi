//! kimchi-interop: working with the other editors people come from.
//!
//! * [`apps`]: what kimchi knows about each app (Premiere Pro, Final Cut Pro, DaVinci Resolve,
//!   CapCut, Kdenlive…): which of its files kimchi opens and writes, how to bring a project over
//!   and back, where its looks are, which of its plugins run here, its keyboard layout, and how
//!   to tell it is installed. The first-run setup and `docs/COMPATIBILITY.md` are built from it.
//! * [`timeline`]: project and timeline files: OpenTimelineIO, FCPXML, Final Cut 7 / Premiere
//!   XML, CMX 3600 EDL, Kdenlive and Shotcut (MLT), OpenShot, Premiere Pro projects, CapCut
//!   drafts. Read into a kimchi [`kimchi_core::Project`], written from one, with a [`Report`] of
//!   what made the trip.
//! * [`looks`]: colour looks and presets: LUT files, Lightroom / Camera Raw presets, Premiere's
//!   Lumetri presets, read as kimchi [`kimchi_core::Effects`].
//!
//! Everything here is plain Rust working on files: no other app is needed, nothing is run.

pub mod apps;
pub mod looks;
pub mod report;
pub mod timeline;

pub use report::Report;

pub type Result<T> = std::result::Result<T, String>;
