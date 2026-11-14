-- What the monitor has seen of a watched account's posts, so it can say which
-- ones are new (`watch_posts.rs`, `snob_core::watch::posts`).
--
-- A post is news when its pk was never seen here **and** it was taken after
-- the newest post seen before (`watch_post_marks.newest_taken_at`, the
-- floor). The second half is what keeps a deletion or an archive from being
-- news: the older posts that move up into the first page of the grid have
-- pks never seen, and they are all older than the floor. The floor is
-- Instagram's `taken_at`, not this machine's clock, so a clock running ahead
-- cannot swallow a post.
--
-- One row per post seen, in the database of the account that reads it (the
-- viewer), like the rest of the monitor's state. Rows older than the
-- captures' thirty days are swept, all but the newest three pages of each
-- account, which is the most one run reads.
CREATE TABLE watch_posts (
  account_pk    INTEGER NOT NULL REFERENCES accounts(pk) ON DELETE CASCADE,
  -- The media's pk, as its decimal digits. TEXT and not INTEGER: it is a u64,
  -- SQLite's integers are signed, and a conversion the database cannot check
  -- is one this schema does not make.
  media_pk      TEXT    NOT NULL CHECK (media_pk <> '' AND media_pk NOT GLOB '*[^0-9]*'),
  code          TEXT    NOT NULL,
  taken_at      INTEGER NOT NULL,
  first_seen_at INTEGER NOT NULL,
  -- NULL: seen while laying the baseline, or seen and older than the floor;
  -- never news. Otherwise when the report that named it was committed.
  reported_at   INTEGER,
  -- For a post that was news: NULL while a file of it is still owed to the
  -- directory its entry saves to, and set once every file is there. A post
  -- that was news on an entry that saves nothing gets its report's moment,
  -- since nothing is owed.
  saved_at      INTEGER,
  PRIMARY KEY (account_pk, media_pk)
) STRICT, WITHOUT ROWID;

-- The receipt for an account's posts: that a baseline exists, and the floor.
--
-- A row of its own rather than "has any post": an account with no post yet
-- would otherwise have no row, and its first post would be taken for a
-- baseline again and never reported.
CREATE TABLE watch_post_marks (
  account_pk      INTEGER NOT NULL PRIMARY KEY REFERENCES accounts(pk) ON DELETE CASCADE,
  baseline_at     INTEGER NOT NULL,
  -- The last read that concluded anything.
  read_at         INTEGER NOT NULL,
  -- The last report that named a post; NULL until one did.
  reported_at     INTEGER,
  -- The floor: the newest `taken_at` ever seen of this account, 0 for an
  -- account with no post yet.
  newest_taken_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
