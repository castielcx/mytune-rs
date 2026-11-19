CREATE TABLE IF NOT EXISTS keys (
  key TEXT PRIMARY KEY,
  note TEXT,
  created INTEGER,
  revoked INTEGER DEFAULT 0,
  device TEXT,
  device_name TEXT,
  bound_at INTEGER,
  last_seen INTEGER
);
