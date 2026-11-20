-- What tells a run still going from one that was killed, and what a run
-- found as data beside its sentences.
--
-- `alive_at` is written when a run starts and again every `RUN_BEAT` while
-- it goes: a row with no end whose last sign of life is older than
-- `RUN_QUIET_AFTER` was stopped before it could write one
-- (`Shared::automation_runs`). NULL in the rows from before it.
--
-- `found` is a JSON array of `snob_engine::automations::said::Said`, what a
-- front words in its own language; NULL in the rows from before it, and in a
-- run that never finished.
ALTER TABLE automation_runs ADD COLUMN alive_at INTEGER;
ALTER TABLE automation_runs ADD COLUMN found TEXT;
