-- External identity (Bitrix24) and server-side ACL. Default DENY. No user secrets stored here.
CREATE TABLE IF NOT EXISTS bitrix_principals (
  member_id TEXT NOT NULL,
  user_id BIGINT NOT NULL CHECK (user_id > 0),
  active BOOLEAN NOT NULL DEFAULT TRUE,
  global_role TEXT NOT NULL DEFAULT 'user' CHECK (global_role IN ('user','admin')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (member_id, user_id)
);
CREATE TABLE IF NOT EXISTS bitrix_package_acl (
  member_id TEXT NOT NULL,
  user_id BIGINT NOT NULL,
  package_id BIGINT NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('viewer','uploader','engineer')),
  PRIMARY KEY (member_id,user_id,package_id),
  FOREIGN KEY(member_id,user_id) REFERENCES bitrix_principals(member_id,user_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS bitrix_package_acl_package_idx ON bitrix_package_acl(package_id);
-- Example manual provisioning, run AFTER replacing placeholders:
-- INSERT INTO bitrix_principals(member_id,user_id,global_role) VALUES ('actual_member_id',143,'admin');
-- INSERT INTO bitrix_principals(member_id,user_id) VALUES ('actual_member_id',281);
-- INSERT INTO bitrix_package_acl(member_id,user_id,package_id,role) VALUES ('actual_member_id',281,1,'viewer');
