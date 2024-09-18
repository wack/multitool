//! Resource can be defined either statically or dynamically.
//! A static resource is one whose definition is hard-coded in Rust.
//! We pre-define all of the resource's CRUDL operations in Rust.
//!
//! In contrast, a dynamic resource is one that's loaded from WebAssembly.
//! The type-checker inspects the Wasm Component's API to determine
//! the resource's schema.
//!
//! At the time of writing, dynamic resources are not supported.

mod static_;
