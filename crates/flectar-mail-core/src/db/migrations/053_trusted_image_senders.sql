-- Senders whose remote images load without asking: the reader's
-- "Always load images" choice for the sender's From address. Kept on this
-- device for every account, like the block list.
CREATE TABLE trusted_image_senders (
  -- Lowercase, as compared with the sender address shown in the reader.
  address TEXT PRIMARY KEY NOT NULL,
  created_at INTEGER NOT NULL
);
