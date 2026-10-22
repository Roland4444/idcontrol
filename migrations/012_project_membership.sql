-- Ordinary user access to a project independent of access to work packages.
-- Does not implicitly grant access to documents or uploads.
-- Existing package ACL entries continue to imply project visibility.
CREATE TABLE IF NOT EXISTS bitrix_project_access (
    member_id TEXT NOT NULL,
    user_id BIGINT NOT NULL,
    project_id BIGINT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    granted_by BIGINT NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(member_id, user_id, project_id),
    FOREIGN KEY(member_id, user_id) REFERENCES bitrix_principals(member_id, user_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS bitrix_project_access_project_idx ON bitrix_project_access(project_id);
