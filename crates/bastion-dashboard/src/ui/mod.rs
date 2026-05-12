//! UI module for the Bastion dashboard.
//!
//! This module provides the Leptos WASM-based user interface components.
//! It uses CSR (client-side rendering) mode for the initial implementation.

#[cfg(feature = "csr")]
pub mod app;
#[cfg(feature = "csr")]
pub mod pages;
#[cfg(feature = "csr")]
pub mod components;
#[cfg(feature = "csr")]
pub mod state;
