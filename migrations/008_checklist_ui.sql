-- Custom checklist entries are scoped to a single package, never to the shared catalog.
CREATE TABLE IF NOT EXISTS package_custom_checklist (
 id BIGSERIAL PRIMARY KEY,
 package_id BIGINT NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
 code TEXT NOT NULL UNIQUE,
 title TEXT NOT NULL CHECK (char_length(title) BETWEEN 2 AND 300),
 required BOOLEAN NOT NULL DEFAULT TRUE,
 active BOOLEAN NOT NULL DEFAULT TRUE,
 note TEXT NOT NULL DEFAULT '',
 updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS package_custom_checklist_package ON package_custom_checklist(package_id,active);
