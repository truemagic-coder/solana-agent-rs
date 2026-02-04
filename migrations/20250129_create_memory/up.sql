CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    timestamp BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_user_time
    ON messages(user_id, timestamp);

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    content,
    user_id,
    message_id UNINDEXED
);

CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content, user_id, message_id)
    VALUES (new.id, new.content, new.user_id, new.id);
END;

CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content, user_id, message_id)
    VALUES('delete', old.id, old.content, old.user_id, old.id);
END;

CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content, user_id, message_id)
    VALUES('delete', old.id, old.content, old.user_id, old.id);
    INSERT INTO messages_fts(rowid, content, user_id, message_id)
    VALUES (new.id, new.content, new.user_id, new.id);
END;

CREATE TABLE IF NOT EXISTS memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    summary TEXT NOT NULL,
    tags TEXT,
    salience REAL,
    created_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memories_user_time
    ON memories(user_id, created_at);

CREATE TABLE IF NOT EXISTS entities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    canonical_id TEXT,
    created_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_entities_user_name
    ON entities(user_id, name);

CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload TEXT,
    occurred_at BIGINT,
    created_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_user_time
    ON events(user_id, created_at);

CREATE TABLE IF NOT EXISTS facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    object TEXT NOT NULL,
    confidence REAL,
    source TEXT,
    created_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_facts_user_subject
    ON facts(user_id, subject);

CREATE TABLE IF NOT EXISTS edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    src_node_type TEXT NOT NULL,
    src_node_id INTEGER NOT NULL,
    dst_node_type TEXT NOT NULL,
    dst_node_id INTEGER NOT NULL,
    edge_type TEXT NOT NULL,
    weight REAL,
    created_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_edges_user_src
    ON edges(user_id, src_node_type, src_node_id, edge_type);

CREATE TABLE IF NOT EXISTS memory_links (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER NOT NULL,
    node_type TEXT NOT NULL,
    node_id INTEGER NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memory_links_memory
    ON memory_links(memory_id, node_type);

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    summary,
    user_id,
    memory_id UNINDEXED
);

CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, summary, user_id, memory_id)
    VALUES (new.id, new.summary, new.user_id, new.id);
END;

CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, summary, user_id, memory_id)
    VALUES('delete', old.id, old.summary, old.user_id, old.id);
END;

CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, summary, user_id, memory_id)
    VALUES('delete', old.id, old.summary, old.user_id, old.id);
    INSERT INTO memories_fts(rowid, summary, user_id, memory_id)
    VALUES (new.id, new.summary, new.user_id, new.id);
END;
