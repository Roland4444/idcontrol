-- ID-Control v0.9.19: employee reference sources; never official contractor deliveries.
CREATE TABLE IF NOT EXISTS reference_materials (
    document_id BIGINT PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    category TEXT NOT NULL CHECK(category IN ('PD','RD','NTD','ORDERS','SRO','PERMIT','QUALITY','JOURNALS','OTHER')),
    note TEXT NOT NULL DEFAULT '',
    uploaded_by TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS reference_materials_category_idx ON reference_materials(category);
