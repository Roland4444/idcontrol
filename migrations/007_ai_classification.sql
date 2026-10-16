-- AI document classification with provider choice per package; never alters originals.
CREATE TABLE IF NOT EXISTS classification_settings (
 package_id BIGINT PRIMARY KEY REFERENCES packages(id) ON DELETE CASCADE,
 provider TEXT NOT NULL DEFAULT 'vibecode' CHECK (provider IN ('vibecode','gigacowork')),
 auto_queue BOOLEAN NOT NULL DEFAULT FALSE,
 updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS classification_jobs (
 id BIGSERIAL PRIMARY KEY,
 package_id BIGINT NOT NULL REFERENCES packages(id),
 document_id BIGINT NOT NULL REFERENCES documents(id),
 provider TEXT NOT NULL CHECK (provider IN ('vibecode','gigacowork')),
 status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','running','done','failed','cancelled')),
 error_message TEXT NOT NULL DEFAULT '',
 requested_by TEXT NOT NULL DEFAULT 'web',
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 started_at TIMESTAMPTZ,
 finished_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS classification_one_active_document
 ON classification_jobs(document_id) WHERE status IN ('pending','running');
CREATE INDEX IF NOT EXISTS classification_jobs_package ON classification_jobs(package_id,id DESC);
CREATE INDEX IF NOT EXISTS classification_jobs_queue ON classification_jobs(status,id);
CREATE TABLE IF NOT EXISTS classification_results (
 document_id BIGINT PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
 job_id BIGINT REFERENCES classification_jobs(id),
 provider TEXT NOT NULL,
 predicted_type TEXT NOT NULL,
 category TEXT NOT NULL DEFAULT '',
 confidence DOUBLE PRECISION NOT NULL CHECK(confidence>=0 AND confidence<=1),
 explanation TEXT NOT NULL DEFAULT '',
 evidence_json JSONB NOT NULL DEFAULT '[]'::jsonb,
 extracted_fields_json JSONB NOT NULL DEFAULT '{}'::jsonb,
 source_text_sha256 TEXT NOT NULL,
 review_status TEXT NOT NULL DEFAULT 'suggested' CHECK(review_status IN ('suggested','confirmed','rejected','manual_override')),
 reviewed_by TEXT,
 reviewed_at TIMESTAMPTZ,
 updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
