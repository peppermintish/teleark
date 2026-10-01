use rusqlite::{Connection, TransactionBehavior};

use crate::{StorageError, StorageResult};

pub(crate) const APPLICATION_ID: u32 = 0x5441_524B; // "TARK"
pub(crate) const LATEST_SCHEMA_VERSION: u32 = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationProgress {
    Detecting,
    Preparing { from: u32, to: u32 },
    Converting { version: u32 },
    Verifying { version: u32 },
    Completed,
}

pub(crate) struct Migration {
    pub version: u32,
    pub sql: &'static str,
}

pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: r#"
CREATE TABLE accounts (
    id                  INTEGER PRIMARY KEY,
    display_name        TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
    created_at_unix_ms  INTEGER NOT NULL,
    updated_at_unix_ms  INTEGER NOT NULL
) STRICT;

CREATE TABLE chats (
    account_id          INTEGER NOT NULL,
    id                  INTEGER NOT NULL,
    title               TEXT NOT NULL CHECK (length(trim(title)) > 0),
    username            TEXT,
    updated_at_unix_ms  INTEGER NOT NULL,
    PRIMARY KEY (account_id, id),
    FOREIGN KEY (account_id) REFERENCES accounts(id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE logical_files (
    id                      INTEGER PRIMARY KEY,
    name                    TEXT NOT NULL CHECK (length(trim(name)) > 0),
    relative_path           TEXT,
    size_bytes              INTEGER NOT NULL CHECK (size_bytes >= 0),
    kind                    TEXT NOT NULL CHECK (kind IN (
                                'video', 'document', 'archive', 'audio',
                                'image', 'disk_image', 'other')),
    mime_type               TEXT,
    extension               TEXT,
    caption                 TEXT,
    source_account_id       INTEGER,
    source_chat_id          INTEGER,
    created_at_unix_ms      INTEGER,
    modified_at_unix_ms     INTEGER,
    remote_state            TEXT NOT NULL CHECK (remote_state IN (
                                'local_only', 'uploading', 'uploaded', 'remote_missing')),
    encryption_state        TEXT NOT NULL CHECK (encryption_state IN (
                                'unencrypted', 'encrypted', 'locked')),
    verification_state      TEXT NOT NULL CHECK (verification_state IN (
                                'unverified', 'verifying', 'verified', 'failed')),
    package_id              INTEGER,
    locally_available       INTEGER NOT NULL DEFAULT 0 CHECK (locally_available IN (0, 1)),
    CHECK ((source_account_id IS NULL) = (source_chat_id IS NULL)),
    FOREIGN KEY (source_account_id, source_chat_id)
        REFERENCES chats(account_id, id) ON DELETE SET NULL
) STRICT;

CREATE INDEX logical_files_modified_keyset
    ON logical_files (modified_at_unix_ms DESC, id DESC);
CREATE INDEX logical_files_source
    ON logical_files (source_account_id, source_chat_id, modified_at_unix_ms DESC, id DESC);
CREATE INDEX logical_files_kind
    ON logical_files (kind, modified_at_unix_ms DESC, id DESC);
CREATE INDEX logical_files_extension
    ON logical_files (extension, modified_at_unix_ms DESC, id DESC);
CREATE INDEX logical_files_size
    ON logical_files (size_bytes, id);

CREATE TABLE settings (
    key                 TEXT PRIMARY KEY CHECK (length(key) BETWEEN 1 AND 128),
    value               TEXT NOT NULL CHECK (length(CAST(value AS BLOB)) <= 1048576),
    updated_at_unix_ms  INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE collections (
    id                  INTEGER PRIMARY KEY,
    name                TEXT NOT NULL CHECK (length(trim(name)) > 0),
    kind                TEXT NOT NULL CHECK (kind IN ('manual', 'smart')),
    rule_version        INTEGER,
    rule_payload        TEXT,
    created_at_unix_ms  INTEGER NOT NULL,
    updated_at_unix_ms  INTEGER NOT NULL,
    CHECK (
        (kind = 'manual' AND rule_version IS NULL AND rule_payload IS NULL)
        OR
        (kind = 'smart' AND rule_version > 0 AND rule_payload IS NOT NULL)
    )
) STRICT;

CREATE TABLE collection_items (
    collection_id      INTEGER NOT NULL,
    logical_file_id    INTEGER NOT NULL,
    added_at_unix_ms   INTEGER NOT NULL,
    PRIMARY KEY (collection_id, logical_file_id),
    FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE,
    FOREIGN KEY (logical_file_id) REFERENCES logical_files(id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE INDEX collection_items_file ON collection_items (logical_file_id, collection_id);
"#,
    },
    Migration {
        version: 2,
        sql: r#"
CREATE VIRTUAL TABLE logical_files_fts USING fts5(
    name,
    relative_path,
    caption,
    content='logical_files',
    content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER logical_files_fts_insert AFTER INSERT ON logical_files BEGIN
    INSERT INTO logical_files_fts(rowid, name, relative_path, caption)
    VALUES (new.id, new.name, new.relative_path, new.caption);
END;

CREATE TRIGGER logical_files_fts_delete AFTER DELETE ON logical_files BEGIN
    INSERT INTO logical_files_fts(logical_files_fts, rowid, name, relative_path, caption)
    VALUES ('delete', old.id, old.name, old.relative_path, old.caption);
END;

CREATE TRIGGER logical_files_fts_update AFTER UPDATE ON logical_files BEGIN
    INSERT INTO logical_files_fts(logical_files_fts, rowid, name, relative_path, caption)
    VALUES ('delete', old.id, old.name, old.relative_path, old.caption);
    INSERT INTO logical_files_fts(rowid, name, relative_path, caption)
    VALUES (new.id, new.name, new.relative_path, new.caption);
END;

INSERT INTO logical_files_fts(logical_files_fts) VALUES ('rebuild');
"#,
    },
    Migration {
        version: 3,
        sql: r#"
CREATE TABLE transfer_tasks (
    id                      INTEGER PRIMARY KEY,
    logical_file_id         INTEGER NOT NULL,
    account_id              INTEGER,
    direction               TEXT NOT NULL CHECK (direction IN ('upload', 'download')),
    priority                INTEGER NOT NULL,
    state                   TEXT NOT NULL CHECK (state IN (
                                'queued', 'running', 'paused', 'waiting_retry',
                                'verifying', 'completed', 'failed', 'cancelled')),
    total_bytes             INTEGER NOT NULL CHECK (total_bytes >= 0),
    transferred_bytes       INTEGER NOT NULL CHECK (
                                transferred_bytes >= 0 AND transferred_bytes <= total_bytes),
    source_path             TEXT,
    destination_path        TEXT,
    retry_count             INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    next_retry_at_unix_ms   INTEGER,
    last_error_code         TEXT,
    created_at_unix_ms      INTEGER NOT NULL,
    updated_at_unix_ms      INTEGER NOT NULL,
    FOREIGN KEY (logical_file_id) REFERENCES logical_files(id) ON DELETE RESTRICT,
    FOREIGN KEY (account_id) REFERENCES accounts(id) ON DELETE SET NULL
) STRICT;

CREATE INDEX transfer_tasks_queue
    ON transfer_tasks (state, priority DESC, created_at_unix_ms, id);
CREATE INDEX transfer_tasks_file
    ON transfer_tasks (logical_file_id, updated_at_unix_ms DESC);

CREATE TABLE transfer_parts (
    transfer_id            INTEGER NOT NULL,
    part_index             INTEGER NOT NULL CHECK (part_index >= 0),
    offset_bytes           INTEGER NOT NULL CHECK (offset_bytes >= 0),
    size_bytes             INTEGER NOT NULL CHECK (size_bytes > 0),
    transferred_bytes      INTEGER NOT NULL CHECK (
                               transferred_bytes >= 0 AND transferred_bytes <= size_bytes),
    state                  TEXT NOT NULL CHECK (state IN (
                               'queued', 'running', 'paused', 'waiting_retry',
                               'transferred', 'verifying', 'verified', 'failed', 'cancelled')),
    attempts               INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    remote_object_id       INTEGER,
    checkpoint_version     INTEGER,
    checkpoint_data        BLOB,
    updated_at_unix_ms     INTEGER NOT NULL,
    PRIMARY KEY (transfer_id, part_index),
    CHECK ((checkpoint_version IS NULL) = (checkpoint_data IS NULL)),
    FOREIGN KEY (transfer_id) REFERENCES transfer_tasks(id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE index_jobs (
    id                          INTEGER PRIMARY KEY,
    account_id                  INTEGER NOT NULL,
    chat_id                     INTEGER NOT NULL,
    state                       TEXT NOT NULL CHECK (state IN (
                                    'queued', 'running', 'paused', 'completed', 'failed', 'cancelled')),
    policy_version              INTEGER NOT NULL CHECK (policy_version > 0),
    policy_fingerprint          TEXT NOT NULL CHECK (length(policy_fingerprint) > 0),
    requested_start_message_id  INTEGER,
    requested_end_message_id    INTEGER,
    checkpoint_message_id       INTEGER,
    messages_scanned            INTEGER NOT NULL DEFAULT 0 CHECK (messages_scanned >= 0),
    files_indexed               INTEGER NOT NULL DEFAULT 0 CHECK (files_indexed >= 0),
    last_error_code             TEXT,
    created_at_unix_ms          INTEGER NOT NULL,
    updated_at_unix_ms          INTEGER NOT NULL,
    CHECK (
        requested_start_message_id IS NULL
        OR requested_end_message_id IS NULL
        OR requested_start_message_id <= requested_end_message_id
    ),
    CHECK (
        checkpoint_message_id IS NULL
        OR requested_start_message_id IS NULL
        OR checkpoint_message_id >= requested_start_message_id
    ),
    CHECK (
        checkpoint_message_id IS NULL
        OR requested_end_message_id IS NULL
        OR checkpoint_message_id <= requested_end_message_id
    ),
    FOREIGN KEY (account_id, chat_id) REFERENCES chats(account_id, id) ON DELETE CASCADE
) STRICT;

CREATE INDEX index_jobs_resume
    ON index_jobs (state, account_id, chat_id, updated_at_unix_ms, id);

CREATE TABLE index_ranges (
    id                      INTEGER PRIMARY KEY,
    account_id              INTEGER NOT NULL,
    chat_id                 INTEGER NOT NULL,
    start_message_id        INTEGER NOT NULL,
    end_message_id          INTEGER NOT NULL,
    coverage                TEXT NOT NULL CHECK (coverage IN ('partial', 'complete')),
    checkpoint_message_id   INTEGER,
    messages_scanned        INTEGER NOT NULL DEFAULT 0 CHECK (messages_scanned >= 0),
    files_indexed           INTEGER NOT NULL DEFAULT 0 CHECK (files_indexed >= 0),
    policy_version          INTEGER NOT NULL CHECK (policy_version > 0),
    policy_fingerprint      TEXT NOT NULL CHECK (length(policy_fingerprint) > 0),
    scan_generation         INTEGER NOT NULL CHECK (scan_generation >= 0),
    updated_at_unix_ms      INTEGER NOT NULL,
    CHECK (start_message_id <= end_message_id),
    CHECK (
        checkpoint_message_id IS NULL
        OR checkpoint_message_id BETWEEN start_message_id AND end_message_id
    ),
    UNIQUE (
        account_id, chat_id, policy_version, policy_fingerprint,
        start_message_id, end_message_id
    ),
    FOREIGN KEY (account_id, chat_id) REFERENCES chats(account_id, id) ON DELETE CASCADE
) STRICT;

CREATE INDEX index_ranges_coverage
    ON index_ranges (
        account_id, chat_id, policy_version, policy_fingerprint,
        start_message_id, end_message_id
    );
"#,
    },
    Migration {
        version: 4,
        sql: r#"
ALTER TABLE logical_files ADD COLUMN local_source_path_encoding TEXT;
ALTER TABLE logical_files ADD COLUMN local_source_path BLOB;

CREATE TRIGGER logical_files_local_path_insert
BEFORE INSERT ON logical_files
WHEN (new.local_source_path_encoding IS NULL) != (new.local_source_path IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'incomplete local source path');
END;

CREATE TRIGGER logical_files_local_path_update
BEFORE UPDATE OF local_source_path_encoding, local_source_path ON logical_files
WHEN (new.local_source_path_encoding IS NULL) != (new.local_source_path IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'incomplete local source path');
END;

CREATE TABLE id_allocators (
    entity      TEXT PRIMARY KEY,
    next_id     INTEGER NOT NULL CHECK (next_id > 0)
) STRICT, WITHOUT ROWID;

INSERT INTO id_allocators (entity, next_id)
SELECT 'logical_file', COALESCE(max(id), 0) + 1 FROM logical_files;
"#,
    },
    Migration {
        version: 5,
        sql: r#"
CREATE TABLE remote_objects (
    id                      INTEGER PRIMARY KEY,
    logical_file_id         INTEGER NOT NULL,
    account_id              INTEGER NOT NULL,
    chat_id                 INTEGER NOT NULL,
    message_id              INTEGER NOT NULL,
    revision                INTEGER NOT NULL CHECK (revision >= 0),
    remote_key              BLOB NOT NULL CHECK (length(remote_key) BETWEEN 1 AND 16384),
    encoded_size_bytes      INTEGER NOT NULL CHECK (encoded_size_bytes >= 0),
    modified_at_unix_ms     INTEGER NOT NULL,
    UNIQUE (account_id, chat_id, message_id),
    FOREIGN KEY (logical_file_id) REFERENCES logical_files(id) ON DELETE CASCADE,
    FOREIGN KEY (account_id, chat_id) REFERENCES chats(account_id, id) ON DELETE CASCADE
) STRICT;

CREATE INDEX remote_objects_file ON remote_objects (logical_file_id, id);
CREATE INDEX remote_objects_source_revision
    ON remote_objects (account_id, chat_id, message_id, revision);

CREATE TABLE telegram_index_state (
    account_id              INTEGER NOT NULL,
    chat_id                 INTEGER NOT NULL,
    before_message_id       INTEGER,
    exhausted               INTEGER NOT NULL CHECK (exhausted IN (0, 1)),
    messages_scanned        INTEGER NOT NULL CHECK (messages_scanned >= 0),
    files_indexed           INTEGER NOT NULL CHECK (files_indexed >= 0),
    updated_at_unix_ms      INTEGER NOT NULL,
    PRIMARY KEY (account_id, chat_id),
    FOREIGN KEY (account_id, chat_id) REFERENCES chats(account_id, id) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

INSERT INTO id_allocators (entity, next_id) VALUES ('remote_object', 1);
"#,
    },
    Migration {
        version: 6,
        sql: r#"
CREATE TABLE native_download_tasks (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id                     INTEGER NOT NULL CHECK (chat_id > 0),
    message_id                  INTEGER NOT NULL CHECK (message_id > 0),
    file_name                   TEXT NOT NULL CHECK (
                                    length(trim(file_name)) > 0
                                    AND length(CAST(file_name AS BLOB)) <= 4096),
    size_bytes                  INTEGER NOT NULL CHECK (size_bytes >= 0),
    destination_path            TEXT NOT NULL UNIQUE CHECK (
                                    length(CAST(destination_path AS BLOB)) BETWEEN 1 AND 32768),
    state                       TEXT NOT NULL CHECK (state IN (
                                    'queued', 'running', 'paused',
                                    'completed', 'failed', 'cancelled')),
    verification                TEXT NOT NULL CHECK (verification IN (
                                    'pending', 'size_checked', 'not_reached')),
    transferred_bytes           INTEGER NOT NULL CHECK (
                                    transferred_bytes >= 0
                                    AND transferred_bytes <= size_bytes),
    started_at_unix_ms          INTEGER,
    finished_at_unix_ms         INTEGER,
    queue_wait_ms               INTEGER CHECK (queue_wait_ms >= 0),
    duration_ms                 INTEGER CHECK (duration_ms >= 0),
    average_bytes_per_second    INTEGER CHECK (average_bytes_per_second >= 0),
    attempts                    INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    failure_code                TEXT CHECK (
                                    failure_code IS NULL
                                    OR length(CAST(failure_code AS BLOB)) BETWEEN 1 AND 64),
    created_at_unix_ms          INTEGER NOT NULL,
    updated_at_unix_ms          INTEGER NOT NULL
) STRICT;

CREATE INDEX native_download_tasks_history
    ON native_download_tasks (created_at_unix_ms DESC, id DESC);
CREATE INDEX native_download_tasks_resume
    ON native_download_tasks (state, created_at_unix_ms, id);
"#,
    },
    Migration {
        version: 7,
        sql: r#"
CREATE TABLE native_download_batches (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id                     INTEGER NOT NULL CHECK (chat_id > 0),
    created_at_unix_ms          INTEGER NOT NULL
) STRICT;

ALTER TABLE native_download_tasks ADD COLUMN batch_id INTEGER
    REFERENCES native_download_batches(id) ON DELETE SET NULL;
ALTER TABLE native_download_tasks ADD COLUMN message_sent_at_unix_ms INTEGER;
ALTER TABLE native_download_tasks ADD COLUMN caption TEXT CHECK (
    caption IS NULL OR length(CAST(caption AS BLOB)) <= 1048576);
ALTER TABLE native_download_tasks ADD COLUMN mime_type TEXT CHECK (
    mime_type IS NULL OR length(CAST(mime_type AS BLOB)) <= 512);

CREATE INDEX native_download_tasks_batch
    ON native_download_tasks (batch_id, id);
CREATE INDEX native_download_batches_history
    ON native_download_batches (created_at_unix_ms DESC, id DESC);
"#,
    },
    Migration {
        version: 8,
        sql: r#"
CREATE TABLE vault_metadata (
    singleton_id            INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    vault_id                BLOB NOT NULL UNIQUE CHECK (length(vault_id) = 16),
    password_wrap           BLOB NOT NULL CHECK (length(password_wrap) BETWEEN 124 AND 172),
    recovery_wrap           BLOB NOT NULL CHECK (length(recovery_wrap) = 88),
    password_generation     INTEGER NOT NULL CHECK (password_generation > 0),
    recovery_generation     INTEGER NOT NULL CHECK (recovery_generation > 0),
    created_at_unix_ms      INTEGER NOT NULL,
    updated_at_unix_ms      INTEGER NOT NULL CHECK (updated_at_unix_ms >= created_at_unix_ms)
) STRICT;
"#,
    },
    Migration {
        version: 9,
        sql: r#"
ALTER TABLE native_download_tasks ADD COLUMN account_id INTEGER
    CHECK (account_id IS NULL OR account_id > 0);
CREATE INDEX native_download_tasks_account_state
    ON native_download_tasks (account_id, state, created_at_unix_ms, id);
"#,
    },
    Migration {
        version: 10,
        sql: r#"
CREATE TABLE vault_downloaded_files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL CHECK (account_id > 0),
    chat_id INTEGER NOT NULL CHECK (chat_id > 0),
    package_id TEXT NOT NULL CHECK (length(package_id) = 32 AND package_id NOT GLOB '*[^0-9a-f]*'),
    path_encoding TEXT NOT NULL CHECK (path_encoding IN ('unix-bytes-v1', 'windows-utf16le-v1', 'utf8-v1')),
    destination_path BLOB NOT NULL CHECK (length(destination_path) BETWEEN 1 AND 32768),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    completed_at_unix_ms INTEGER NOT NULL,
    UNIQUE (account_id, path_encoding, destination_path)
) STRICT;
CREATE INDEX vault_downloaded_files_account ON vault_downloaded_files (account_id, id);
CREATE INDEX native_download_completed_outputs ON native_download_tasks (account_id, id)
    WHERE state = 'completed';
"#,
    },
    Migration {
        version: 11,
        sql: r#"
CREATE TABLE channel_sync_state (
    account_id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL,
    pts INTEGER NOT NULL CHECK (pts >= 0),
    history_before INTEGER,
    history_exhausted INTEGER NOT NULL CHECK (history_exhausted IN (0, 1)),
    repair_pending INTEGER NOT NULL CHECK (repair_pending IN (0, 1)),
    repair_before INTEGER,
    gap_pending INTEGER NOT NULL CHECK (gap_pending IN (0, 1)),
    gap_before INTEGER,
    gap_until INTEGER,
    revision INTEGER NOT NULL CHECK (revision > 0),
    PRIMARY KEY (account_id, chat_id),
    FOREIGN KEY (account_id, chat_id) REFERENCES chats(account_id, id)
) STRICT, WITHOUT ROWID;
CREATE TABLE channel_sync_tombstones (
    account_id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL,
    PRIMARY KEY (account_id, chat_id, message_id),
    FOREIGN KEY (account_id, chat_id) REFERENCES chats(account_id, id)
) STRICT, WITHOUT ROWID;
"#,
    },
    Migration {
        version: 12,
        sql: r#"
CREATE TABLE channel_file_versions (
    account_id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL CHECK (message_id > 0),
    revision INTEGER NOT NULL CHECK (revision > 0),
    PRIMARY KEY (account_id, chat_id, message_id),
    FOREIGN KEY (account_id, chat_id) REFERENCES chats(account_id, id)
) STRICT, WITHOUT ROWID;
-- This is an observation scope, never proof of remote identity or write authority.
CREATE TABLE managed_channel_watches (
    account_id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL,
    catalog_ready INTEGER NOT NULL DEFAULT 0 CHECK (catalog_ready IN (0, 1)),
    change_count INTEGER NOT NULL DEFAULT 0 CHECK (change_count >= 0),
    acknowledged_count INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged_count >= 0 AND acknowledged_count <= change_count),
    last_changed_at INTEGER,
    PRIMARY KEY (account_id, chat_id),
    FOREIGN KEY (account_id, chat_id) REFERENCES chats(account_id, id)
) STRICT, WITHOUT ROWID;
CREATE TABLE managed_channel_changes (
    account_id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    message_id INTEGER NOT NULL CHECK (message_id >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('edited', 'deleted', 'gap')),
    pts INTEGER NOT NULL CHECK (pts >= 0),
    observed_at INTEGER NOT NULL,
    PRIMARY KEY (account_id, chat_id, sequence),
    UNIQUE (account_id, chat_id, message_id, kind, pts),
    FOREIGN KEY (account_id, chat_id) REFERENCES managed_channel_watches(account_id, chat_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX channel_manifest_candidates ON logical_files (source_account_id, source_chat_id, caption, created_at_unix_ms DESC);
"#,
    },
    Migration {
        version: 13,
        sql: r#"
-- Search orders by the effective timestamp, including created-at/null fallback.
-- Keep the earlier raw modified-at indexes for explicit timestamp-range facets.
CREATE INDEX logical_files_effective_keyset ON logical_files
    (COALESCE(modified_at_unix_ms, created_at_unix_ms, -9223372036854775808) DESC, id DESC);
CREATE INDEX logical_files_account_effective_keyset ON logical_files
    (source_account_id, COALESCE(modified_at_unix_ms, created_at_unix_ms, -9223372036854775808) DESC, id DESC);
CREATE INDEX logical_files_source_effective_keyset ON logical_files
    (source_account_id, source_chat_id, COALESCE(modified_at_unix_ms, created_at_unix_ms, -9223372036854775808) DESC, id DESC);
CREATE INDEX logical_files_kind_effective_keyset ON logical_files
    (kind, COALESCE(modified_at_unix_ms, created_at_unix_ms, -9223372036854775808) DESC, id DESC);
"#,
    },
    Migration {
        version: 14,
        sql: r#"
-- Fence readers that predate mandatory proxy routing. A missing policy after
-- this migration is corruption, not permission to create direct connections.
INSERT INTO settings (key, value, updated_at_unix_ms)
VALUES ('network.proxy', '{"version":1,"mode":"direct"}', 0);
"#,
    },
    Migration {
        version: 15,
        sql: r#"
-- Key epochs are independent of password/recovery wrap generations. The
-- singleton remains the active upload epoch; old wrapped keys remain readable.
CREATE TABLE vault_key_epochs (
    vault_id BLOB PRIMARY KEY CHECK (length(vault_id) = 16),
    password_wrap BLOB NOT NULL CHECK (length(password_wrap) BETWEEN 124 AND 172),
    recovery_wrap BLOB NOT NULL CHECK (length(recovery_wrap) = 88),
    password_generation INTEGER NOT NULL CHECK (password_generation > 0),
    recovery_generation INTEGER NOT NULL CHECK (recovery_generation > 0),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL CHECK (updated_at_unix_ms >= created_at_unix_ms)
) STRICT, WITHOUT ROWID;
INSERT INTO vault_key_epochs SELECT vault_id,password_wrap,recovery_wrap,
    password_generation,recovery_generation,created_at_unix_ms,updated_at_unix_ms
    FROM vault_metadata;
CREATE TABLE vault_inventory (
    account_id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL,
    manifest_message_id INTEGER NOT NULL CHECK (manifest_message_id > 0),
    remote_name TEXT NOT NULL CHECK (length(remote_name) <= 128),
    vault_id BLOB NOT NULL CHECK (length(vault_id) = 16),
    sealed_manifest BLOB NOT NULL CHECK (length(sealed_manifest) BETWEEN 1 AND 16777216),
    observed_at INTEGER NOT NULL,
    health_scan_run INTEGER NOT NULL DEFAULT 0,
    manifest_invalid INTEGER NOT NULL DEFAULT 0 CHECK (manifest_invalid IN (0,1)),
    PRIMARY KEY(account_id,chat_id,manifest_message_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX vault_inventory_name ON vault_inventory(account_id,chat_id,remote_name,manifest_message_id DESC);
CREATE TABLE vault_message_health (
    account_id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL CHECK (message_id > 0),
    present INTEGER NOT NULL CHECK (present IN (0,1)),
    observed_at INTEGER NOT NULL,
    PRIMARY KEY(account_id,chat_id,message_id)
) STRICT, WITHOUT ROWID;
"#,
    },
    Migration {
        version: 16,
        sql: r#"
CREATE TABLE vault_upload_history (
    sequence INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL CHECK (account_id > 0),
    id INTEGER NOT NULL UNIQUE CHECK (id > 0),
    chat_id INTEGER NOT NULL CHECK (chat_id > 0),
    batch_id INTEGER CHECK (batch_id > 0),
    queued_at INTEGER NOT NULL CHECK (queued_at >= 0),
    file_name TEXT NOT NULL CHECK (length(file_name) BETWEEN 1 AND 4096),
    package_id TEXT,
    size_bytes INTEGER NOT NULL CHECK (size_bytes > 0),
    transferred_bytes INTEGER NOT NULL CHECK (transferred_bytes BETWEEN 0 AND size_bytes),
    completed_parts INTEGER NOT NULL CHECK (completed_parts >= 0),
    part_count INTEGER NOT NULL CHECK (part_count >= completed_parts),
    started_at INTEGER NOT NULL CHECK (started_at >= 0),
    duration_ms INTEGER CHECK (duration_ms >= 0),
    average_bps INTEGER CHECK (average_bps >= 0),
    state TEXT NOT NULL CHECK (state IN ('queued','running','completed','failed','cancelled','interrupted')),
    failure_code TEXT,
    CHECK ((state = 'failed') = (failure_code IS NOT NULL)),
    CHECK (state != 'completed' OR (package_id IS NOT NULL AND transferred_bytes = size_bytes AND completed_parts = part_count AND part_count > 0)),
    UNIQUE (account_id, id)
) STRICT;
CREATE INDEX vault_upload_history_account ON vault_upload_history(account_id, sequence DESC);
CREATE INDEX vault_upload_history_batch ON vault_upload_history(account_id, batch_id, sequence DESC);
"#,
    },
    Migration {
        version: 17,
        sql: r#"
CREATE TABLE vault_transfer_jobs (
    account_id INTEGER NOT NULL CHECK (account_id > 0),
    id INTEGER NOT NULL CHECK (id > 0),
    chat_id INTEGER NOT NULL CHECK (chat_id > 0),
    direction TEXT NOT NULL CHECK (direction IN ('upload','download')),
    package_id BLOB NOT NULL CHECK (length(package_id) = 16),
    context_version INTEGER NOT NULL CHECK (context_version > 0),
    context BLOB NOT NULL CHECK (length(context) BETWEEN 1 AND 2097152),
    state TEXT NOT NULL CHECK (state IN ('queued','running','pausing','paused','cancelling','cancelled','retryable','blocked','completed')),
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    failure_code TEXT CHECK (length(failure_code) BETWEEN 1 AND 64),
    CHECK ((state IN ('retryable','blocked')) = (failure_code IS NOT NULL)),
    PRIMARY KEY (account_id,id)
) STRICT, WITHOUT ROWID;
CREATE INDEX vault_transfer_jobs_schedule ON vault_transfer_jobs(account_id,state,id);
CREATE TABLE vault_transfer_parts (
    account_id INTEGER NOT NULL,
    task_id INTEGER NOT NULL,
    part_index INTEGER NOT NULL CHECK (part_index >= 0),
    identity BLOB NOT NULL CHECK (length(identity) BETWEEN 1 AND 16384),
    receipt BLOB CHECK (length(receipt) BETWEEN 1 AND 16384),
    PRIMARY KEY (account_id,task_id,part_index),
    FOREIGN KEY (account_id,task_id) REFERENCES vault_transfer_jobs(account_id,id)
) STRICT, WITHOUT ROWID;
"#,
    },
    Migration {
        version: 18,
        sql: r#"
CREATE TABLE vault_manifest_outbox (
    account_id INTEGER NOT NULL,
    task_id INTEGER NOT NULL,
    codec_version INTEGER NOT NULL CHECK (codec_version > 0),
    commitment BLOB NOT NULL CHECK (length(commitment) = 32),
    random_id INTEGER NOT NULL CHECK (random_id != 0),
    envelope BLOB CHECK (length(envelope) BETWEEN 1 AND 67239936),
    message_id INTEGER CHECK (message_id > 0),
    CHECK (message_id IS NULL OR envelope IS NOT NULL),
    PRIMARY KEY (account_id,task_id),
    FOREIGN KEY (account_id,task_id) REFERENCES vault_transfer_jobs(account_id,id)
) STRICT, WITHOUT ROWID;
"#,
    },
    Migration {
        version: 19,
        sql: "CREATE INDEX vault_transfer_jobs_direction ON vault_transfer_jobs(account_id,direction,id);",
    },
    Migration {
        version: 20,
        sql: r#"
CREATE TABLE vault_pending_uploads (
    account_id INTEGER NOT NULL CHECK (account_id > 0),
    id INTEGER NOT NULL CHECK (id > 0),
    chat_id INTEGER NOT NULL CHECK (chat_id > 0),
    batch_id INTEGER NOT NULL CHECK (batch_id > 0),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    codec_version INTEGER NOT NULL CHECK (codec_version > 0),
    context BLOB NOT NULL CHECK (length(context) BETWEEN 1 AND 131072),
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    state TEXT NOT NULL CHECK (state IN ('queued','paused','cancelled','retryable','blocked','promoted')),
    failure_code TEXT CHECK (length(failure_code) BETWEEN 1 AND 64),
    CHECK ((state IN ('retryable','blocked')) = (failure_code IS NOT NULL)),
    PRIMARY KEY (account_id,id)
) STRICT, WITHOUT ROWID;
CREATE INDEX vault_pending_uploads_schedule ON vault_pending_uploads(account_id,state,id);
"#,
    },
    Migration {
        version: 21,
        sql: r#"
CREATE TABLE native_download_cleanup (
    task_id INTEGER PRIMARY KEY REFERENCES native_download_tasks(id) ON DELETE RESTRICT,
    codec_version INTEGER NOT NULL DEFAULT 1 CHECK (codec_version > 0),
    attempt INTEGER NOT NULL CHECK (attempt >= 0),
    requested_at INTEGER NOT NULL CHECK (requested_at >= 0),
    retry_requested INTEGER NOT NULL DEFAULT 0 CHECK (retry_requested IN (0,1))
) STRICT;
"#,
    },
    Migration {
        version: 22,
        sql: "CREATE INDEX vault_pending_uploads_batch ON vault_pending_uploads(account_id,batch_id,state,id); CREATE INDEX vault_transfer_jobs_history ON vault_transfer_jobs(account_id,direction,CASE WHEN state IN ('queued','running','pausing','cancelling') THEN 0 WHEN state IN ('paused','retryable','blocked') THEN 1 ELSE 2 END,id DESC);",
    },
    Migration {
        version: 23,
        sql: r#"
CREATE TABLE credential_backend (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    keychain_enabled INTEGER NOT NULL CHECK (keychain_enabled IN (0,1)),
    revision INTEGER NOT NULL CHECK (revision >= 0)
) STRICT;
CREATE TABLE credential_items (
    namespace TEXT NOT NULL CHECK (length(namespace) BETWEEN 1 AND 128),
    identity TEXT NOT NULL CHECK (length(identity) BETWEEN 1 AND 512),
    codec_version INTEGER NOT NULL CHECK (codec_version = 1),
    keychain_account TEXT NOT NULL CHECK (length(keychain_account) BETWEEN 1 AND 640),
    payload BLOB CHECK (length(payload) BETWEEN 1 AND 65536),
    PRIMARY KEY (namespace,identity)
) STRICT, WITHOUT ROWID;
CREATE TABLE credential_cleanup (
    namespace TEXT NOT NULL CHECK (length(namespace) BETWEEN 1 AND 128),
    keychain_account TEXT NOT NULL CHECK (length(keychain_account) BETWEEN 1 AND 640),
    PRIMARY KEY (namespace,keychain_account)
) STRICT, WITHOUT ROWID;
"#,
    },
    Migration {
        version: 24,
        sql: "ALTER TABLE channel_sync_state ADD COLUMN last_synced_at INTEGER CHECK (last_synced_at >= 0);",
    },
];

pub(crate) fn migrate(
    connection: &mut Connection,
    progress: &impl Fn(MigrationProgress),
) -> StorageResult<()> {
    progress(MigrationProgress::Detecting);
    let application_id = read_pragma_u32(connection, "application_id")?;
    let current_version = read_pragma_u32(connection, "user_version")?;

    if current_version > LATEST_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedSchema {
            found: current_version,
            latest: LATEST_SCHEMA_VERSION,
        });
    }
    if application_id != 0 && application_id != APPLICATION_ID {
        return Err(StorageError::WrongApplication {
            found: application_id,
        });
    }
    if current_version != 0 && application_id == 0 {
        return Err(StorageError::WrongApplication { found: 0 });
    }

    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > current_version)
    {
        progress(MigrationProgress::Preparing {
            from: migration.version - 1,
            to: migration.version,
        });
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if migration.version == 1 {
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
        }
        progress(MigrationProgress::Converting {
            version: migration.version,
        });
        transaction.execute_batch(migration.sql)?;
        progress(MigrationProgress::Verifying {
            version: migration.version,
        });
        if migration.version == LATEST_SCHEMA_VERSION {
            let check: String =
                transaction.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
            let violations: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_check",
                [],
                |row| row.get(0),
            )?;
            if check != "ok" || violations != 0 {
                return Err(StorageError::CorruptData {
                    entity: "migration",
                    field: "integrity",
                    value: "verification_failed".into(),
                });
            }
        }
        transaction.pragma_update(None, "user_version", migration.version)?;
        transaction.commit()?;
    }
    progress(MigrationProgress::Completed);
    Ok(())
}

fn read_pragma_u32(connection: &Connection, pragma: &str) -> StorageResult<u32> {
    let value: i64 = connection.pragma_query_value(None, pragma, |row| row.get(0))?;
    u32::try_from(value).map_err(|_| StorageError::CorruptData {
        entity: "database",
        field: "pragma",
        value: value.to_string(),
    })
}
