-- Breach verdicts for sender addresses, from Have I Been Pwned's paid
-- breach lookup. One row per address: the addresses checked in the last
-- week, and the list of breaches each one appears in. A clean sender is a
-- row with an empty list, so its 404 is not asked about again for a week.
CREATE TABLE sender_breaches (
  -- Lowercase, as compared with the sender address shown in the list.
  address TEXT PRIMARY KEY NOT NULL,
  checked_at INTEGER NOT NULL,
  -- The HIBP breach list as JSON; '[]' for a sender with no known breach.
  breach_json TEXT NOT NULL
);
