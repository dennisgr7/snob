-- When a run was due, and when it began, beside when it ended
-- (`finished_at`): so a run that started late can be told from one on time,
-- whether the machine slept through its moment, a timer missed it, or it
-- waited for the budget or a cooldown.
--
-- `due_at` is the moment the schedule said the run was for, as the
-- evaluator worked it out (`schedule::due_moment`, or the moment the loop
-- slept towards); NULL for a run started by hand, which was due for
-- nothing, and for every run before this. `began_at` is when the run began,
-- before any of its work; `started_at` stays what it always was, the moment
-- its comparison was made, which the schedule's floor counts from.
ALTER TABLE watch_runs ADD COLUMN due_at INTEGER;
ALTER TABLE watch_runs ADD COLUMN began_at INTEGER;
