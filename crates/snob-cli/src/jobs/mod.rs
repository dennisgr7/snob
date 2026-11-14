//! Jobs as the command line has them: the engine's (`snob_engine::jobs`),
//! with the halves tied to its grammar, the start of a job and the check of
//! a job's command line, and the commands that read jobs back.

pub use snob_engine::jobs::*;

pub mod argv;
pub mod dispatch;
pub mod start;
