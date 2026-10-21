//! Exercise the real private profile and atomic storage without starting the desktop.
#![allow(dead_code)]
#[path = "../src/persona/self_profile.rs"]
pub mod self_profile;
#[path = "../src/persona/self_profile_store.rs"]
pub mod self_profile_store;
#[path = "../src/utils/fs.rs"]
pub mod fs;
pub mod persona { pub use crate::self_profile; }
pub mod utils { pub use crate::fs; }
