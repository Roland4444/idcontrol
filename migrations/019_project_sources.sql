-- v0.9.22 Project-wide design sources. One immutable archive, many package references.
-- No existing contractor deliveries, documents, or approvals are deleted.
CREATE TABLE IF NOT EXISTS project_sources (
 id BIGSERIAL PRIMARY KEY,
 project_id BIGINT NOT NULL REFERENCES projects(id),
 category TEXT NOT NULL CHECK(category IN ('KR','PZU','POS','PPR','PD','RD','SURVEYS','NTD','OTHER')),
 filename TEXT NOT NULL,
 revision INTEGER NOT NULL CHECK(revision >= 1),
 sha256 TEXT NOT NULL CHECK(length(sha256)=64),
 archive_path TEXT NOT NULL,
 nas_path TEXT,
 bytes BIGINT NOT NULL,
 note TEXT NOT NULL DEFAULT '',
 uploaded_by TEXT NOT NULL,
 recognized_text TEXT NOT NULL DEFAULT '',
 recognition_status TEXT NOT NULL DEFAULT 'pending',
 recognition_confidence REAL,
 status TEXT NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','processing','ready','error')),
 error_message TEXT,
 is_current BOOLEAN NOT NULL DEFAULT false,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 processed_at TIMESTAMPTZ,
 UNIQUE(project_id,category,filename,revision)
);
CREATE INDEX IF NOT EXISTS project_sources_scope ON project_sources(project_id,category,filename,revision DESC);
CREATE UNIQUE INDEX IF NOT EXISTS project_sources_current ON project_sources(project_id,category,filename) WHERE is_current;
CREATE TABLE IF NOT EXISTS project_source_package_documents (
 project_source_id BIGINT NOT NULL REFERENCES project_sources(id),
 document_id BIGINT NOT NULL UNIQUE REFERENCES documents(id),
 PRIMARY KEY(project_source_id,document_id)
);
