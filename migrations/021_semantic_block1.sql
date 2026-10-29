-- 0.9.25: Semantics are suggestions with verified source quotes, never an automatic sign-off.
-- Additive; no change to contractor originals, previous runs, old migrations or approved rules.
ALTER TABLE project_requirement_candidates ADD COLUMN IF NOT EXISTS extraction_method TEXT NOT NULL DEFAULT 'heuristic';
ALTER TABLE project_requirement_candidates ADD COLUMN IF NOT EXISTS semantic_payload JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE project_requirement_candidates ADD COLUMN IF NOT EXISTS ai_model TEXT;
CREATE TABLE IF NOT EXISTS block1_semantic_jobs (
 id BIGSERIAL PRIMARY KEY,
 package_id BIGINT NOT NULL REFERENCES packages(id),
 run_id BIGINT REFERENCES check_runs(id),
 job_kind TEXT NOT NULL CHECK(job_kind IN ('extract','compare')),
 status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','running','ready','error')),
 model TEXT NOT NULL,
 total INTEGER NOT NULL DEFAULT 0,
 completed INTEGER NOT NULL DEFAULT 0,
 found INTEGER NOT NULL DEFAULT 0,
 error_message TEXT,
 created_by TEXT NOT NULL,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 started_at TIMESTAMPTZ,
 finished_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS block1_semantic_active_job
 ON block1_semantic_jobs(package_id,job_kind) WHERE status IN ('pending','running');
CREATE INDEX IF NOT EXISTS block1_semantic_jobs_history ON block1_semantic_jobs(package_id,created_at DESC);
CREATE TABLE IF NOT EXISTS block1_semantic_evidence (
 id BIGSERIAL PRIMARY KEY,
 job_id BIGINT NOT NULL REFERENCES block1_semantic_jobs(id),
 run_id BIGINT NOT NULL REFERENCES check_runs(id),
 package_id BIGINT NOT NULL REFERENCES packages(id),
 candidate_id BIGINT NOT NULL REFERENCES project_requirement_candidates(id),
 document_id BIGINT REFERENCES documents(id),
 verdict TEXT NOT NULL CHECK(verdict IN ('supported','conflict','insufficient','unverified')),
 source_quote TEXT NOT NULL,
 document_quote TEXT NOT NULL DEFAULT '',
 explanation TEXT NOT NULL DEFAULT '',
 model TEXT NOT NULL,
 source_signature CHAR(64) NOT NULL,
 facts JSONB NOT NULL DEFAULT '{}'::jsonb,
 reviewed_by TEXT,
 review_status TEXT NOT NULL DEFAULT 'pending' CHECK(review_status IN ('pending','confirmed','rejected')),
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 UNIQUE(job_id,candidate_id)
);
CREATE INDEX IF NOT EXISTS block1_semantic_evidence_run ON block1_semantic_evidence(run_id,candidate_id);
