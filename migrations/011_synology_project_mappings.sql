-- v0.9.4: explicit NAS project-folder -> six fixed housing projects mapping.
-- No existing document/scan records are moved or deleted.
CREATE TABLE IF NOT EXISTS synology_project_mappings (
    source_name TEXT PRIMARY KEY,
    project_id BIGINT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    default_work TEXT NOT NULL DEFAULT '',
    changed_by TEXT NOT NULL DEFAULT '',
    changed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (char_length(source_name) BETWEEN 1 AND 200),
    CHECK (char_length(default_work) <= 200)
);
