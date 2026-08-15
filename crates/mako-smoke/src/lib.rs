//! End-to-end smoke verification for the Mako Cloud application happy path.
//!
//! The behaviour under test lives entirely in `tests/happy_path.rs`, which
//! drives the real service binaries over HTTP. This crate carries no library
//! code of its own; it exists to own that test target, which cannot live beside
//! either service because it needs binaries from both.

#![forbid(unsafe_code)]
