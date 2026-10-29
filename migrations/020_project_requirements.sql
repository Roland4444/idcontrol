-- v0.9.24: source-grounded Block-1 proposals, engineer decisions and frozen approvals.
-- Additive only. Original PDFs/DOCs, revisions, delivery metadata and check runs stay untouched.
CREATE TABLE IF NOT EXISTS project_requirement_candidates (
    id BIGSERIAL PRIMARY KEY,
    project_id BIGINT NOT NULL REFERENCES projects(id),
    source_id BIGINT NOT NULL REFERENCES project_sources(id),
    catalog_name TEXT NOT NULL,
    catalog_code TEXT NOT NULL,
    title TEXT NOT NULL,
    evidence_excerpt TEXT NOT NULL,
    source_page INTEGER CHECK (source_page IS NULL OR source_page > 0),
    source_line INTEGER NOT NULL CHECK (source_line > 0),
    extraction_kind TEXT NOT NULL CHECK(extraction_kind IN ('document','project_condition','withdrawn')),
    confidence REAL NOT NULL CHECK(confidence BETWEEN 0 AND 1),
    fingerprint CHAR(64) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(source_id,catalog_name,catalog_code,fingerprint)
);
CREATE INDEX IF NOT EXISTS project_requirement_candidate_lookup
 ON project_requirement_candidates(project_id,catalog_name,source_id);
CREATE TABLE IF NOT EXISTS package_project_requirement_decisions (
    package_id BIGINT NOT NULL REFERENCES packages(id),
    candidate_id BIGINT NOT NULL REFERENCES project_requirement_candidates(id),
    disposition TEXT NOT NULL CHECK(disposition IN ('required','not_applicable','review')),
    min_count INTEGER NOT NULL DEFAULT 1 CHECK(min_count BETWEEN 1 AND 999),
    decided_by TEXT NOT NULL,
    decided_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(package_id,candidate_id)
);
ALTER TABLE packages ADD COLUMN IF NOT EXISTS project_requirements_approved_at TIMESTAMPTZ;
ALTER TABLE packages ADD COLUMN IF NOT EXISTS project_requirements_approved_by TEXT;
CREATE TABLE IF NOT EXISTS package_project_requirement_approvals (
    id BIGSERIAL PRIMARY KEY,
    package_id BIGINT NOT NULL REFERENCES packages(id),
    source_signature CHAR(64) NOT NULL,
    snapshot JSONB NOT NULL,
    approved_by TEXT NOT NULL,
    approved_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS project_requirement_generation_jobs (
    project_id BIGINT PRIMARY KEY REFERENCES projects(id),
    status TEXT NOT NULL CHECK(status IN ('running','ready','error')),
    found_count INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);

-- Freeze the exact project requirement basis used by each check run.
CREATE TABLE IF NOT EXISTS check_run_project_requirements (
    run_id BIGINT PRIMARY KEY REFERENCES check_runs(id),
    source_signature CHAR(64) NOT NULL,
    signed BOOLEAN NOT NULL,
    snapshot JSONB NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
