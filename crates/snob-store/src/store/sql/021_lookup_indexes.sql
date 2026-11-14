-- Three lookups that read a whole table to answer, each given the index it
-- seeks, and one index nothing could use taken away.
--
-- An account by its name (`users::pk_named`, `accounts::find_pk_by_username`)
-- is compared `COLLATE NOCASE`, so that a name finds its account in any case.
-- `users_username`, from 001, is in the default binary order, which such a
-- comparison cannot seek: both statements scanned `users`, a row for every
-- account ever seen on any page of any list. The index is made again in the
-- order the comparison uses.
--
-- **The binary one was checked before it was dropped, rather than reasoned
-- about**, as 010 was: no statement in the workspace compares
-- `users.username` in the binary order or sorts by it (the only other read of
-- the column is by `pk`), so it answered no question and was a second copy of
-- every name. `store::migrate` runs `VACUUM` once afterwards, which is what
-- gives its pages back.
DROP INDEX IF EXISTS users_username;
CREATE INDEX users_username_nocase ON users(username COLLATE NOCASE);

-- `snob changes` asks for the renames noticed between two moments
-- (`watch::renames_between`). The index 001 gave the history starts with the
-- account, which a window of time cannot seek, and the history is never
-- pruned.
CREATE INDEX username_history_changed ON username_history(changed_at);

-- When the monitor last started, for any account (`watch::last_started`), and
-- the sweep of runs past their keeping (`watch::prune`). The index 002 gave
-- the runs starts with the account. The rowid an index ends with breaks the
-- tie between two runs started in one second, as the statement asks.
CREATE INDEX watch_runs_started ON watch_runs(started_at);
