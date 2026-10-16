-- Hybrid AI mode. Safe upgrade: all existing projects, files and checks remain untouched.
-- Outbound AI use is opt-in per package and also requires AI_MODE != off.
ALTER TABLE packages ADD COLUMN IF NOT EXISTS ai_consent BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE packages ADD COLUMN IF NOT EXISTS ai_consent_updated_at TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS ai_jobs (
 id BIGSERIAL PRIMARY KEY,
 package_id BIGINT NOT NULL REFERENCES packages(id),
 provider TEXT NOT NULL CHECK(provider IN ('gigachat','cowork_gateway')),
 status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','running','completed','failed')),
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 started_at TIMESTAMPTZ,
 finished_at TIMESTAMPTZ,
 requested_by TEXT NOT NULL,
 input_documents INTEGER,
 input_truncated BOOLEAN,
 suggestions_created INTEGER,
 error_message TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS ai_jobs_package_idx ON ai_jobs(package_id,id DESC);
CREATE UNIQUE INDEX IF NOT EXISTS ai_jobs_one_active_per_package
 ON ai_jobs(package_id) WHERE status IN ('pending','running');

CREATE TABLE IF NOT EXISTS ai_suggestions (
 id BIGSERIAL PRIMARY KEY,
 job_id BIGINT NOT NULL REFERENCES ai_jobs(id),
 package_id BIGINT NOT NULL REFERENCES packages(id),
 document_id BIGINT NOT NULL REFERENCES documents(id),
 reference_document_id BIGINT REFERENCES documents(id),
 source_quote TEXT NOT NULL,
 reference_quote TEXT NOT NULL DEFAULT '',
 description TEXT NOT NULL,
 proposed_action TEXT NOT NULL,
 model_score REAL,
 review_status TEXT NOT NULL DEFAULT 'pending' CHECK(review_status IN ('pending','confirmed','rejected')),
 reviewed_by TEXT,
 reviewed_at TIMESTAMPTZ,
 comment TEXT NOT NULL DEFAULT '',
 created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ai_suggestions_package_idx ON ai_suggestions(package_id,id DESC);
