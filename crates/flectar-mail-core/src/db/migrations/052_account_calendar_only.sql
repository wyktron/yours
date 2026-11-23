-- An account added for its calendar only keeps syncing calendars, but is
-- hidden from Mail: no account or folder rows, no mail lists or counts, and
-- no mail synchronization. Calendar connections are unaffected.
ALTER TABLE accounts ADD COLUMN calendar_only INTEGER NOT NULL DEFAULT 0;
