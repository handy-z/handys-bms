//! Everything but the entry point, so it can be unit-tested.
#![cfg(windows)]

pub mod app {
    pub mod config;
    pub mod offset_keys;
}
pub mod game {
    pub mod board;
}
pub mod r#macro {
    pub mod clicker;
}
pub mod overlay {
    pub mod engine;
    pub mod marks;
}
pub mod solver {
    pub mod replay;
    #[allow(clippy::module_inception)]
    pub mod solver;
}
