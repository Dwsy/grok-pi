-- D1 schema for extension crash telemetry (privacy: name + package_dir only)

CREATE TABLE IF NOT EXISTS reports (
  id TEXT PRIMARY KEY,
  created_at TEXT NOT NULL,
  ext_name TEXT NOT NULL,
  package_dir TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('crash', 'combo', 'unknown')),
  client TEXT,
  grok_pi_ver TEXT
);

CREATE INDEX IF NOT EXISTS idx_reports_ext ON reports(ext_name);
CREATE INDEX IF NOT EXISTS idx_reports_pkg ON reports(package_dir);
CREATE INDEX IF NOT EXISTS idx_reports_created ON reports(created_at);
CREATE INDEX IF NOT EXISTS idx_reports_kind ON reports(kind);

CREATE TABLE IF NOT EXISTS triage (
  package_dir TEXT PRIMARY KEY,
  status TEXT NOT NULL DEFAULT 'open'
    CHECK (status IN ('open', 'blocked', 'wontfix', 'fixed')),
  note TEXT,
  updated_at TEXT NOT NULL
);
