-- What the AI model made of a message: NULL until it was judged, then
-- 'clean', 'suspicious' or 'phishing', with the reason and when it was judged.
ALTER TABLE messages ADD COLUMN security_verdict TEXT;
ALTER TABLE messages ADD COLUMN security_reason TEXT;
ALTER TABLE messages ADD COLUMN security_checked_at INTEGER;
ALTER TABLE messages ADD COLUMN security_model TEXT;

-- How the message actually arrived (Authentication-Results, Received hops,
-- Return-Path), captured at parse time for the phishing screen only.
ALTER TABLE messages ADD COLUMN security_evidence TEXT;

-- The scanner looks for recent mail that has not been judged yet.
CREATE INDEX idx_messages_security_pending ON messages(date DESC)
  WHERE security_checked_at IS NULL;
