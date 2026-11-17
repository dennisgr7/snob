-- What each automation's runs came to, for the app's history of it and for
-- whether one is due: `watch.toml`'s (`watch`) and the ones kept as files.
-- An automation reads as several accounts, so its runs are the machine's.
-- The newest 100 of each are kept (`Shared::record_automation_run`).
CREATE TABLE automation_runs (
  id         INTEGER PRIMARY KEY,
  automation TEXT    NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at   INTEGER,           -- NULL while it runs, or when it was killed
  outcome    TEXT,              -- the exit token, as `watch_runs.outcome`
  requests   INTEGER NOT NULL DEFAULT 0,
  summary    TEXT    NOT NULL DEFAULT ''  -- plain sentences, one a line
) STRICT;

CREATE INDEX automation_runs_of ON automation_runs (automation, started_at);
