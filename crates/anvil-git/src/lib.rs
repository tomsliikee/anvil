//! Git through the host's `git`, so the user's credentials, config and
//! hooks apply: command lines for every operation of the Git Changes and
//! Git Repository windows, and parsers for git's machine-readable output.

pub mod blame;
pub mod log;
pub mod ops;
pub mod status;
