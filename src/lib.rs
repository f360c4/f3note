//! f3note — a fast, minimal tabbed editor for Wayland.
//!
//! The crate is split into a library and a thin binary so that the parts worth
//! testing (palette resolution, stylesheet generation, session and backup
//! bookkeeping) can be exercised without a display server, and so development
//! tools like `xcheck` can reuse them.

pub mod atomic;
pub mod config;
pub mod paths;
pub mod theme;
