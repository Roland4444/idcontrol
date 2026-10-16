-- OCR job queue and bounded human-readable progress events.
-- Does not alter archived originals or any existing document version.
CREATE TABLE IF NOT EXISTS ocr_jobs (
  id BIGSERIAL PRIMARY KEY,
  document_id BIGINT NOT NULL REFERENCES documents(id),
  package_id BIGINT NOT NULL REFERENCES packages(id),
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending','running','done','partial','failed','cancelled')),
  stage TEXT NOT NULL DEFAULT 'queued',
  current_step INTEGER NOT NULL DEFAULT 0,
  total_steps INTEGER NOT NULL DEFAULT 0,
  message TEXT NOT NULL DEFAULT 'Ожидает обработки',
  error_message TEXT NOT NULL DEFAULT '',
  requested_by TEXT NOT NULL DEFAULT 'web',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  started_at TIMESTAMPTZ,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  finished_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS ocr_jobs_pkg_idx ON ocr_jobs(package_id,id DESC);
CREATE INDEX IF NOT EXISTS ocr_jobs_status_idx ON ocr_jobs(status,id);
CREATE UNIQUE INDEX IF NOT EXISTS ocr_jobs_one_active_document
  ON ocr_jobs(document_id) WHERE status IN ('pending','running');
CREATE TABLE IF NOT EXISTS ocr_job_events (
  id BIGSERIAL PRIMARY KEY,
  job_id BIGINT NOT NULL REFERENCES ocr_jobs(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  stage TEXT NOT NULL,
  current_step INTEGER NOT NULL DEFAULT 0,
  total_steps INTEGER NOT NULL DEFAULT 0,
  message TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ocr_job_events_recent_idx ON ocr_job_events(job_id,id DESC);
