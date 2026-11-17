-- What an automation's action found last, as the account that read it: what
-- its "changes" filter compares against. One row per action, the accounts it
-- found as a JSON array of their pks (`automation_marks.rs`).
CREATE TABLE automation_marks (
  automation TEXT    NOT NULL,
  node       INTEGER NOT NULL,
  taken_at   INTEGER NOT NULL,
  members    TEXT    NOT NULL,
  PRIMARY KEY (automation, node)
) STRICT, WITHOUT ROWID;
