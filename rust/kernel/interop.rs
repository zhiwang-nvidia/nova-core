// SPDX-License-Identifier: GPL-2.0

//! Infrastructure for interfacing Rust code with C kernel subsystems.
//!
//! This module is intended for low-level, unsafe Rust infrastructure code
//! that interoperates between Rust and C. Drivers should normally use the
//! generated adapters and safe subsystem abstractions built on top of it.

pub mod ffi;
pub mod list;
