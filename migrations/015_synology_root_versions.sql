-- Stage 2.3: persistent history of administrator-selected ResData roots.
-- No documents, submissions or previously applied migrations are modified.
CREATE TABLE IF NOT EXISTS synology_root_versions (
    id BIGSERIAL PRIMARY KEY,
    version BIGINT NOT NULL UNIQUE CHECK (version > 0),
    root_path TEXT NOT NULL,
    changed_by TEXT NOT NULL,
    changed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Preserve a root selected in v0.9.11/v0.9.12 as the initial version.
INSERT INTO synology_root_versions(version,root_path,changed_by,changed_at)
SELECT 1, setting_value, 'previous_configuration', updated_at
FROM app_settings
WHERE setting_key='synology_root'
  AND NOT EXISTS (SELECT 1 FROM synology_root_versions);
