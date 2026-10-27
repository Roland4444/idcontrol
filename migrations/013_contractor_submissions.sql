-- Stage 2.3: server-selected Synology root and one-time contractor deliveries.
-- Additive only: no existing project, document, or ACL data is altered.
CREATE TABLE IF NOT EXISTS app_settings (
    setting_key TEXT PRIMARY KEY,
    setting_value TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS contractor_upload_links (
    id BIGSERIAL PRIMARY KEY,
    token_hash CHAR(64) NOT NULL UNIQUE,
    package_id BIGINT NOT NULL REFERENCES packages(id),
    created_by TEXT NOT NULL,
    nas_root TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    claimed_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS contractor_links_package ON contractor_upload_links(package_id,created_at DESC);
CREATE TABLE IF NOT EXISTS contractor_submissions (
    id BIGSERIAL PRIMARY KEY,
    link_id BIGINT NOT NULL UNIQUE REFERENCES contractor_upload_links(id),
    package_id BIGINT NOT NULL REFERENCES packages(id),
    revision INTEGER NOT NULL,
    archive_sha256 CHAR(64) NOT NULL,
    archive_filename TEXT NOT NULL,
    nas_path TEXT,
    manifest_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    status TEXT NOT NULL CHECK(status IN ('uploading','pending_review','checking','checked','rework','failed')),
    error_message TEXT,
    check_run_id BIGINT REFERENCES check_runs(id),
    checked_by TEXT,
    uploaded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    reviewed_at TIMESTAMPTZ,
    UNIQUE(package_id,revision)
);
CREATE INDEX IF NOT EXISTS contractor_submissions_package ON contractor_submissions(package_id,id DESC);
