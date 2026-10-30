CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
INSERT INTO schema_migrations(version) VALUES(1);
CREATE TABLE conversations(id TEXT PRIMARY KEY,title TEXT NOT NULL DEFAULT '',provider TEXT NOT NULL DEFAULT '',url TEXT,updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE messages(id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,parent_id TEXT,role TEXT NOT NULL,content TEXT NOT NULL DEFAULT '');
CREATE VIRTUAL TABLE conversations_fts USING fts5(id UNINDEXED,title,content='');
INSERT INTO conversations(id,title,provider) VALUES('legacy-v1','v1 title','legacy');
INSERT INTO messages(id,conversation_id,role,content) VALUES('legacy-message','legacy-v1','user','v1 searchable');
