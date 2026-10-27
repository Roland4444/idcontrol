-- Stage 2.3.1: durable progress for contractor uploads.
-- Migration 013 remains unchanged, preserving SQLx checksums.
ALTER TABLE contractor_submissions
    ADD COLUMN IF NOT EXISTS processing_stage TEXT NOT NULL DEFAULT 'queued',
    ADD COLUMN IF NOT EXISTS processing_percent SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS processed_bytes BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS processing_total_bytes BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS progress_updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
