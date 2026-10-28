-- 0.9.21: verified work scope + optional act-specific supporting documents.
-- No existing file, contractor submission, or historical finding is modified.
CREATE TABLE IF NOT EXISTS package_work_operations (
    id BIGSERIAL PRIMARY KEY,
    package_id BIGINT NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
    description TEXT NOT NULL CHECK(length(trim(description)) BETWEEN 3 AND 400),
    location TEXT NOT NULL DEFAULT '',
    catalog_code TEXT NOT NULL,
    required_count INTEGER NOT NULL CHECK(required_count BETWEEN 1 AND 999),
    basis TEXT NOT NULL CHECK(length(trim(basis)) >= 3),
    active BOOLEAN NOT NULL DEFAULT true,
    added_by TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS package_work_operations_pkg ON package_work_operations(package_id,active);
ALTER TABLE packages ADD COLUMN IF NOT EXISTS block1_scope_approved_by TEXT;
ALTER TABLE packages ADD COLUMN IF NOT EXISTS block1_scope_approved_at TIMESTAMPTZ;
ALTER TABLE package_support_requirements ADD COLUMN IF NOT EXISTS parent_document_id BIGINT REFERENCES documents(id);

-- Evidence must be scoped to a specific area/stage of executed works.
CREATE TABLE IF NOT EXISTS package_work_evidence (
    operation_id BIGINT NOT NULL REFERENCES package_work_operations(id) ON DELETE CASCADE,
    document_id BIGINT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    added_by TEXT NOT NULL,
    added_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(operation_id,document_id)
);
CREATE INDEX IF NOT EXISTS package_work_evidence_document_idx ON package_work_evidence(document_id);

-- Immutable quantitative snapshot for each verification run, not recomputed
-- retroactively when a checklist or an official contractor revision changes.
CREATE TABLE IF NOT EXISTS check_run_block1_metrics (
    run_id BIGINT PRIMARY KEY REFERENCES check_runs(id) ON DELETE CASCADE,
    required_total INTEGER NOT NULL,
    confirmed_total INTEGER NOT NULL,
    coverage_pct INTEGER CHECK(coverage_pct BETWEEN 0 AND 100),
    ready_for_engineer_review BOOLEAN NOT NULL DEFAULT FALSE,
    measured_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
