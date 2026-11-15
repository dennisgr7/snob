//! The halves of jobs tied to the command line's grammar: the start of a job
//! (`start`), the check of a job's command line (`argv`), and the commands
//! that read jobs back (`dispatch`). The rest of jobs is the engine's
//! (`snob_engine::jobs`).

pub mod argv;
pub mod dispatch;
pub mod start;
