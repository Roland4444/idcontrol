-- Bitrix24 SSO sessions are opaque, hashed server-side and time-bound.
CREATE TABLE IF NOT EXISTS portal_sessions (
 token_hash TEXT PRIMARY KEY,
 member_id TEXT NOT NULL,
 user_id BIGINT NOT NULL CHECK (user_id > 0),
 display_name TEXT NOT NULL DEFAULT '',
 expires_at TIMESTAMPTZ NOT NULL,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 FOREIGN KEY(member_id,user_id) REFERENCES bitrix_principals(member_id,user_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS portal_sessions_expiry ON portal_sessions(expires_at);
-- Managers are assigned to individual fixed projects by IDs 1 or 336.
CREATE TABLE IF NOT EXISTS bitrix_project_managers (
 member_id TEXT NOT NULL,
 user_id BIGINT NOT NULL,
 project_id BIGINT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 granted_by BIGINT NOT NULL,
 granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 PRIMARY KEY(member_id,user_id,project_id),
 FOREIGN KEY(member_id,user_id) REFERENCES bitrix_principals(member_id,user_id) ON DELETE CASCADE
);
INSERT INTO projects(name) VALUES
 ('Федерация'), ('OKLAND'), ('TETRIS'), ('КОСМО'), ('PRIME TOWER'), ('DUO')
ON CONFLICT(name) DO NOTHING;
