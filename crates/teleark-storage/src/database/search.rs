use rusqlite::params_from_iter;
use rusqlite::types::Value;

use super::{
    Database, encryption_state_code, file_kind_code, remote_state_code, row_to_logical_file,
    unsigned_to_sql, verification_state_code,
};
use crate::error::{CursorError, InputReason};
use crate::model::{FileSearchFacets, LibrarySourceRecord, PageCursor, SearchPage, SearchQuery};
use crate::{StorageError, StorageResult};

const MAX_PAGE_SIZE: u32 = 500;
const CURSOR_VERSION: u32 = 1;
const SORT_NULL_SENTINEL: i64 = i64::MIN;

const SEARCH_COLUMNS: &str = r#"
    f.id, f.name, f.relative_path, f.size_bytes, f.kind, f.mime_type, f.extension, f.caption,
    f.source_account_id, f.source_chat_id, f.created_at_unix_ms, f.modified_at_unix_ms,
    f.remote_state, f.encryption_state, f.verification_state, f.package_id, f.locally_available,
    f.local_source_path_encoding, f.local_source_path
"#;

#[derive(Clone, Copy)]
struct DecodedCursor {
    sort_value: i64,
    id: i64,
    fingerprint: u64,
}

impl Database {
    /// Searches FTS5 and structured facets with keyset pagination.
    ///
    /// User text is bound as a parameter and escaped as an FTS phrase; facet
    /// values are also parameters. Only repository-owned SQL fragments are composed.
    pub fn search_files(&self, query: &SearchQuery) -> StorageResult<SearchPage> {
        validate_query(query)?;
        let fingerprint = query_fingerprint(query)?;
        let cursor = query.cursor.as_ref().map(decode_cursor).transpose()?;
        if cursor.is_some_and(|cursor| cursor.fingerprint != fingerprint) {
            return Err(StorageError::InvalidCursor(CursorError::QueryMismatch));
        }

        let fts_query = query.text.as_deref().and_then(fts_phrase);
        let mut sql = format!(
            "SELECT {SEARCH_COLUMNS}, COALESCE(f.modified_at_unix_ms, f.created_at_unix_ms, {SORT_NULL_SENTINEL}) AS sort_value,
             (SELECT title FROM chats WHERE account_id = f.source_account_id AND id = f.source_chat_id),
             (SELECT MIN(r.message_id) FROM remote_objects r
              WHERE r.logical_file_id = f.id AND r.account_id = f.source_account_id
                AND r.chat_id = f.source_chat_id AND f.package_id IS NULL
              HAVING COUNT(*) = 1)
             FROM logical_files f"
        );
        let mut count_sql = String::from("SELECT COUNT(*) FROM logical_files f");
        if fts_query.is_some() {
            sql.push_str(" JOIN logical_files_fts ON logical_files_fts.rowid = f.id");
            count_sql.push_str(" JOIN logical_files_fts ON logical_files_fts.rowid = f.id");
        }
        sql.push_str(" WHERE 1 = 1");
        count_sql.push_str(" WHERE 1 = 1");

        let mut values = Vec::<Value>::new();
        let mut count_values = Vec::<Value>::new();
        if let Some(fts_query) = fts_query {
            push_clause(
                &mut sql,
                &mut values,
                "logical_files_fts MATCH ?",
                Value::Text(fts_query.clone()),
            );
            push_clause(
                &mut count_sql,
                &mut count_values,
                "logical_files_fts MATCH ?",
                Value::Text(fts_query),
            );
        }
        add_facets(&mut sql, &mut values, &query.facets)?;
        add_facets(&mut count_sql, &mut count_values, &query.facets)?;
        // A constant WHERE disables SQLite's dedicated Count opcode. Keep the
        // unfiltered shape bare so counting uses B-tree page metadata, while
        // filtered/FTS searches still evaluate their exact predicate.
        if count_sql.ends_with(" WHERE 1 = 1") {
            count_sql.truncate(count_sql.len() - " WHERE 1 = 1".len());
        }
        let total_matching_sql: i64 = self.connection.query_row(
            &count_sql,
            params_from_iter(count_values.iter()),
            |row| row.get(0),
        )?;
        let total_matching =
            u64::try_from(total_matching_sql).map_err(|_| StorageError::CorruptData {
                entity: "search",
                field: "total_matching",
                value: total_matching_sql.to_string(),
            })?;
        if let Some(cursor) = cursor {
            // The scalar upper bound makes SQLite seek the expression index;
            // the tuple alone can still scan every preceding key on deep pages.
            sql.push_str(&format!(
                " AND COALESCE(f.modified_at_unix_ms, f.created_at_unix_ms, {SORT_NULL_SENTINEL}) <= ? AND (COALESCE(f.modified_at_unix_ms, f.created_at_unix_ms, {SORT_NULL_SENTINEL}), f.id) < (?, ?)"
            ));
            values.push(Value::Integer(cursor.sort_value));
            values.push(Value::Integer(cursor.sort_value));
            values.push(Value::Integer(cursor.id));
        }
        sql.push_str(&format!(
            " ORDER BY sort_value DESC, f.id DESC LIMIT {}",
            query.limit + 1
        ));

        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(values.iter()))?;
        let mut found = Vec::with_capacity(query.limit as usize + 1);
        let mut sources = std::collections::BTreeMap::new();
        while let Some(row) = rows.next()? {
            let file = row_to_logical_file(row)?;
            let sort_value: i64 = row.get(19)?;
            let raw_id: i64 = row.get(0)?;
            if let Some(name) = row.get::<_, Option<String>>(20)? {
                sources.insert(
                    file.id,
                    LibrarySourceRecord {
                        name,
                        message_id: row
                            .get::<_, Option<i64>>(21)?
                            .map(teleark_core::MessageId::new),
                    },
                );
            }
            found.push((file, sort_value, raw_id));
        }

        let has_more = found.len() > query.limit as usize;
        if has_more {
            if let Some((extra, _, _)) = found.last() {
                sources.remove(&extra.id);
            }
            found.truncate(query.limit as usize);
        }
        let next_cursor = if has_more {
            found
                .last()
                .map(|(_, sort_value, raw_id)| encode_cursor(*sort_value, *raw_id, fingerprint))
        } else {
            None
        };

        Ok(SearchPage {
            files: found.into_iter().map(|(file, _, _)| file).collect(),
            sources,
            next_cursor,
            total_matching,
        })
    }
}

fn validate_query(query: &SearchQuery) -> StorageResult<()> {
    if query.limit == 0 || query.limit > MAX_PAGE_SIZE {
        return Err(StorageError::InvalidInput {
            field: "search.limit",
            reason: InputReason::OutOfRange,
        });
    }
    if query.facets.chat_id.is_some() && query.facets.account_id.is_none() {
        return Err(StorageError::InvalidInput {
            field: "search.facets.chat_id",
            reason: InputReason::InvalidCombination,
        });
    }
    if query
        .facets
        .minimum_size_bytes
        .zip(query.facets.maximum_size_bytes)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(StorageError::InvalidInput {
            field: "search.facets.size",
            reason: InputReason::InvalidCombination,
        });
    }
    if query
        .facets
        .modified_from_unix_ms
        .zip(query.facets.modified_through_unix_ms)
        .is_some_and(|(start, end)| start > end)
    {
        return Err(StorageError::InvalidInput {
            field: "search.facets.modified_at",
            reason: InputReason::InvalidCombination,
        });
    }
    Ok(())
}

fn push_clause(sql: &mut String, values: &mut Vec<Value>, clause: &str, value: Value) {
    sql.push_str(" AND ");
    sql.push_str(clause);
    values.push(value);
}

fn add_facets(
    sql: &mut String,
    values: &mut Vec<Value>,
    facets: &FileSearchFacets,
) -> StorageResult<()> {
    if let Some(account_id) = facets.account_id {
        push_clause(
            sql,
            values,
            "f.source_account_id = ?",
            Value::Integer(account_id.get()),
        );
    }
    if let Some(chat_id) = facets.chat_id {
        push_clause(
            sql,
            values,
            "f.source_chat_id = ?",
            Value::Integer(chat_id.get()),
        );
    }
    if let Some(kind) = facets.kind {
        push_clause(
            sql,
            values,
            "f.kind = ?",
            Value::Text(file_kind_code(kind)?.to_owned()),
        );
    }
    if let Some(extension) = facets.extension.as_deref() {
        let extension = extension.trim().trim_start_matches('.').to_lowercase();
        if extension.is_empty() {
            return Err(StorageError::InvalidInput {
                field: "search.facets.extension",
                reason: InputReason::Empty,
            });
        }
        push_clause(
            sql,
            values,
            "lower(f.extension) = ?",
            Value::Text(extension),
        );
    }
    if let Some(size) = facets.minimum_size_bytes {
        push_clause(
            sql,
            values,
            "f.size_bytes >= ?",
            Value::Integer(unsigned_to_sql("search.minimum_size_bytes", size)?),
        );
    }
    if let Some(size) = facets.maximum_size_bytes {
        push_clause(
            sql,
            values,
            "f.size_bytes <= ?",
            Value::Integer(unsigned_to_sql("search.maximum_size_bytes", size)?),
        );
    }
    if let Some(timestamp) = facets.modified_from_unix_ms {
        push_clause(
            sql,
            values,
            "f.modified_at_unix_ms >= ?",
            Value::Integer(timestamp),
        );
    }
    if let Some(timestamp) = facets.modified_through_unix_ms {
        push_clause(
            sql,
            values,
            "f.modified_at_unix_ms <= ?",
            Value::Integer(timestamp),
        );
    }
    if let Some(state) = facets.remote_state {
        push_clause(
            sql,
            values,
            "f.remote_state = ?",
            Value::Text(remote_state_code(state)?.to_owned()),
        );
    }
    if let Some(state) = facets.encryption_state {
        push_clause(
            sql,
            values,
            "f.encryption_state = ?",
            Value::Text(encryption_state_code(state)?.to_owned()),
        );
    }
    if let Some(state) = facets.verification_state {
        push_clause(
            sql,
            values,
            "f.verification_state = ?",
            Value::Text(verification_state_code(state)?.to_owned()),
        );
    }
    if let Some(multipart) = facets.multipart {
        push_clause(
            sql,
            values,
            if multipart {
                "f.package_id IS NOT NULL AND ? = 1"
            } else {
                "f.package_id IS NULL AND ? = 0"
            },
            Value::Integer(i64::from(multipart)),
        );
    }
    if let Some(available) = facets.locally_available {
        push_clause(
            sql,
            values,
            "f.locally_available = ?",
            Value::Integer(i64::from(available)),
        );
    }
    Ok(())
}

fn fts_phrase(input: &str) -> Option<String> {
    let input = input.trim();
    (!input.is_empty()).then(|| format!("\"{}\"", input.replace('"', "\"\"")))
}

fn encode_cursor(sort_value: i64, id: i64, fingerprint: u64) -> PageCursor {
    let sort_bits = u64::from_be_bytes(sort_value.to_be_bytes());
    PageCursor(format!(
        "ta{CURSOR_VERSION}:{sort_bits:016x}:{id:016x}:{fingerprint:016x}"
    ))
}

fn decode_cursor(cursor: &PageCursor) -> StorageResult<DecodedCursor> {
    let mut parts = cursor.as_str().split(':');
    let version = parts
        .next()
        .and_then(|value| value.strip_prefix("ta"))
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or(StorageError::InvalidCursor(CursorError::Malformed))?;
    if version != CURSOR_VERSION {
        return Err(StorageError::InvalidCursor(
            CursorError::UnsupportedVersion { version },
        ));
    }
    let sort_bits = parse_hex(parts.next())?;
    let id_bits = parse_hex(parts.next())?;
    let fingerprint = parse_hex(parts.next())?;
    if parts.next().is_some() {
        return Err(StorageError::InvalidCursor(CursorError::Malformed));
    }
    let id =
        i64::try_from(id_bits).map_err(|_| StorageError::InvalidCursor(CursorError::Malformed))?;
    Ok(DecodedCursor {
        sort_value: i64::from_be_bytes(sort_bits.to_be_bytes()),
        id,
        fingerprint,
    })
}

fn parse_hex(value: Option<&str>) -> StorageResult<u64> {
    let value = value.ok_or(StorageError::InvalidCursor(CursorError::Malformed))?;
    if value.len() != 16 {
        return Err(StorageError::InvalidCursor(CursorError::Malformed));
    }
    u64::from_str_radix(value, 16).map_err(|_| StorageError::InvalidCursor(CursorError::Malformed))
}

fn query_fingerprint(query: &SearchQuery) -> StorageResult<u64> {
    let mut hash = Fnv1a::new();
    hash.field(query.text.as_deref().unwrap_or("").trim().as_bytes());
    let facets = &query.facets;
    hash.field(
        &facets
            .account_id
            .map_or(i64::MIN, |id| id.get())
            .to_be_bytes(),
    );
    hash.field(&facets.chat_id.map_or(i64::MIN, |id| id.get()).to_be_bytes());
    hash.field(
        facets
            .kind
            .map(file_kind_code)
            .transpose()?
            .unwrap_or("")
            .as_bytes(),
    );
    hash.field(
        facets
            .extension
            .as_deref()
            .unwrap_or("")
            .trim()
            .trim_start_matches('.')
            .to_lowercase()
            .as_bytes(),
    );
    hash.optional_u64(facets.minimum_size_bytes);
    hash.optional_u64(facets.maximum_size_bytes);
    hash.optional_i64(facets.modified_from_unix_ms);
    hash.optional_i64(facets.modified_through_unix_ms);
    hash.field(
        facets
            .remote_state
            .map(remote_state_code)
            .transpose()?
            .unwrap_or("")
            .as_bytes(),
    );
    hash.field(
        facets
            .encryption_state
            .map(encryption_state_code)
            .transpose()?
            .unwrap_or("")
            .as_bytes(),
    );
    hash.field(
        facets
            .verification_state
            .map(verification_state_code)
            .transpose()?
            .unwrap_or("")
            .as_bytes(),
    );
    hash.optional_bool(facets.multipart);
    hash.optional_bool(facets.locally_available);
    Ok(hash.finish())
}

struct Fnv1a(u64);

impl Fnv1a {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn field(&mut self, bytes: &[u8]) {
        self.bytes(&(bytes.len() as u64).to_be_bytes());
        self.bytes(bytes);
    }

    fn optional_u64(&mut self, value: Option<u64>) {
        self.field(&value.map_or_else(
            || vec![0],
            |value| {
                let mut bytes = vec![1];
                bytes.extend(value.to_be_bytes());
                bytes
            },
        ));
    }

    fn optional_i64(&mut self, value: Option<i64>) {
        self.field(&value.map_or_else(
            || vec![0],
            |value| {
                let mut bytes = vec![1];
                bytes.extend(value.to_be_bytes());
                bytes
            },
        ));
    }

    fn optional_bool(&mut self, value: Option<bool>) {
        self.field(&[match value {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        }]);
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    const fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_rejects_malformed_data_without_panicking() {
        for value in ["", "ta2:0:0:0", "ta1:not-hex:0:0", "ta1:0000000000000000"] {
            assert!(decode_cursor(&PageCursor(value.to_owned())).is_err());
        }
    }

    #[test]
    fn cursor_preserves_negative_sort_keys() -> StorageResult<()> {
        let cursor = encode_cursor(-42, 7, 11);
        let decoded = decode_cursor(&cursor)?;
        assert_eq!(decoded.sort_value, -42);
        assert_eq!(decoded.id, 7);
        assert_eq!(decoded.fingerprint, 11);
        Ok(())
    }

    #[test]
    fn fts_input_is_one_escaped_phrase() {
        assert_eq!(
            fts_phrase("name\" OR *"),
            Some("\"name\"\" OR *\"".to_owned())
        );
        assert_eq!(fts_phrase("  "), None);
    }
}
