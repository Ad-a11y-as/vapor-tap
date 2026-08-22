// Derived from flexaudio-os-windows 0.2.0 (MIT, Studio Sadola).
#![allow(unsafe_op_in_unsafe_fn)]

mod common;
mod process;
mod system;

pub(super) use process::WasapiProcessBackend;
pub(super) use system::{WasapiSystemBackend, list_output_devices};
