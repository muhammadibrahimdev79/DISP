use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAGIC: &[u8; 8] = b"DISPDB\x1a\n";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 32;
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TABLES: usize = 4096;
const MAX_FIELDS: usize = 4096;
const MAX_ROWS: usize = 100_000;
const MAX_NAME_BYTES: usize = 1024;
const MAX_TYPE_BYTES: usize = 128;
const MAX_ROW_BYTES: usize = 16 * 1024 * 1024;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub tables: Vec<Table>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Table {
    pub name: String,
    pub fields: Vec<Field>,
    pub rows: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Field {
    pub name: String,
    pub storage: String,
    pub optional: bool,
    pub primary: bool,
}

impl Snapshot {
    pub(crate) fn empty() -> Self {
        Self { tables: Vec::new() }
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    })
}

fn push_u32(output: &mut Vec<u8>, value: usize, context: &str) -> io::Result<()> {
    let value = u32::try_from(value).map_err(|_| invalid(format!("{context} is too large")))?;
    output.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn push_u64(output: &mut Vec<u8>, value: usize, context: &str) -> io::Result<()> {
    let value = u64::try_from(value).map_err(|_| invalid(format!("{context} is too large")))?;
    output.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn push_bytes(output: &mut Vec<u8>, value: &[u8], limit: usize, context: &str) -> io::Result<()> {
    if value.len() > limit {
        return Err(invalid(format!("{context} exceeds its storage limit")));
    }
    push_u32(output, value.len(), context)?;
    output.extend_from_slice(value);
    Ok(())
}

pub(crate) fn encode(snapshot: &Snapshot) -> io::Result<Vec<u8>> {
    if snapshot.tables.len() > MAX_TABLES {
        return Err(invalid("DISP Data snapshot exceeds 4096 tables"));
    }
    let mut tables = snapshot.tables.iter().collect::<Vec<_>>();
    tables.sort_by(|left, right| left.name.cmp(&right.name));
    let mut table_names = HashSet::new();
    let mut payload = Vec::new();
    push_u32(&mut payload, tables.len(), "table count")?;
    for table in tables {
        if !table_names.insert(table.name.as_str()) {
            return Err(invalid("DISP Data snapshot contains a duplicate table"));
        }
        push_bytes(
            &mut payload,
            table.name.as_bytes(),
            MAX_NAME_BYTES,
            "table name",
        )?;
        if table.fields.is_empty() || table.fields.len() > MAX_FIELDS {
            return Err(invalid(
                "DISP Data table must contain between 1 and 4096 fields",
            ));
        }
        push_u32(&mut payload, table.fields.len(), "field count")?;
        let mut field_names = HashSet::new();
        let mut primary_count = 0;
        for field in &table.fields {
            if !field_names.insert(field.name.as_str()) {
                return Err(invalid("DISP Data table contains a duplicate field"));
            }
            push_bytes(
                &mut payload,
                field.name.as_bytes(),
                MAX_NAME_BYTES,
                "field name",
            )?;
            push_bytes(
                &mut payload,
                field.storage.as_bytes(),
                MAX_TYPE_BYTES,
                "field storage type",
            )?;
            payload.push(u8::from(field.optional) | (u8::from(field.primary) << 1));
            primary_count += usize::from(field.primary);
        }
        if primary_count != 1 {
            return Err(invalid(
                "DISP Data table must contain exactly one primary field",
            ));
        }
        if table.rows.len() > MAX_ROWS {
            return Err(invalid("DISP Data table exceeds 100000 rows"));
        }
        push_u64(&mut payload, table.rows.len(), "row count")?;
        for row in &table.rows {
            push_bytes(&mut payload, row, MAX_ROW_BYTES, "stored row")?;
        }
        if payload.len() > MAX_FILE_BYTES - HEADER_SIZE {
            return Err(invalid("DISP Data snapshot exceeds 64 MiB"));
        }
    }

    let mut output = Vec::with_capacity(HEADER_SIZE + payload.len());
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&VERSION.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    output.extend_from_slice(&fnv1a(&payload).to_le_bytes());
    output.extend_from_slice(&payload);
    Ok(output)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize, context: &str) -> io::Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| invalid(format!("DISP Data snapshot is truncated in {context}")))?;
        let value = &self.bytes[self.at..end];
        self.at = end;
        Ok(value)
    }

    fn u8(&mut self, context: &str) -> io::Result<u8> {
        Ok(self.take(1, context)?[0])
    }

    fn u32(&mut self, context: &str) -> io::Result<usize> {
        let bytes: [u8; 4] = self.take(4, context)?.try_into().unwrap();
        Ok(u32::from_le_bytes(bytes) as usize)
    }

    fn u64(&mut self, context: &str) -> io::Result<usize> {
        let bytes: [u8; 8] = self.take(8, context)?.try_into().unwrap();
        usize::try_from(u64::from_le_bytes(bytes))
            .map_err(|_| invalid(format!("{context} does not fit this target")))
    }

    fn text(&mut self, limit: usize, context: &str) -> io::Result<String> {
        let length = self.u32(context)?;
        if length > limit {
            return Err(invalid(format!("{context} exceeds its storage limit")));
        }
        String::from_utf8(self.take(length, context)?.to_vec())
            .map_err(|_| invalid(format!("{context} is not valid UTF-8")))
    }

    fn data(&mut self, limit: usize, context: &str) -> io::Result<Vec<u8>> {
        let length = self.u32(context)?;
        if length > limit {
            return Err(invalid(format!("{context} exceeds its storage limit")));
        }
        Ok(self.take(length, context)?.to_vec())
    }
}

pub(crate) fn decode(bytes: &[u8]) -> io::Result<Snapshot> {
    if bytes.len() < HEADER_SIZE {
        return Err(invalid("DISP Data snapshot header is truncated"));
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(invalid("DISP Data snapshot exceeds 64 MiB"));
    }
    if &bytes[..8] != MAGIC {
        return Err(invalid("file is not a DISP Data snapshot"));
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if version != VERSION {
        return Err(invalid(format!(
            "unsupported DISP Data snapshot version {version}"
        )));
    }
    if u32::from_le_bytes(bytes[12..16].try_into().unwrap()) != 0 {
        return Err(invalid("DISP Data snapshot uses unknown required flags"));
    }
    let payload_length = usize::try_from(u64::from_le_bytes(bytes[16..24].try_into().unwrap()))
        .map_err(|_| invalid("DISP Data snapshot length does not fit this target"))?;
    if payload_length != bytes.len() - HEADER_SIZE {
        return Err(invalid("DISP Data snapshot length is inconsistent"));
    }
    let payload = &bytes[HEADER_SIZE..];
    let expected = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    if fnv1a(payload) != expected {
        return Err(invalid("DISP Data snapshot integrity check failed"));
    }

    let mut reader = Reader {
        bytes: payload,
        at: 0,
    };
    let table_count = reader.u32("table count")?;
    if table_count > MAX_TABLES {
        return Err(invalid("DISP Data snapshot exceeds 4096 tables"));
    }
    let mut tables = Vec::with_capacity(table_count);
    let mut table_names = HashSet::new();
    for _ in 0..table_count {
        let name = reader.text(MAX_NAME_BYTES, "table name")?;
        if !table_names.insert(name.clone()) {
            return Err(invalid("DISP Data snapshot contains a duplicate table"));
        }
        let field_count = reader.u32("field count")?;
        if field_count == 0 || field_count > MAX_FIELDS {
            return Err(invalid(
                "DISP Data table must contain between 1 and 4096 fields",
            ));
        }
        let mut fields = Vec::with_capacity(field_count);
        let mut field_names = HashSet::new();
        let mut primary_count = 0;
        for _ in 0..field_count {
            let name = reader.text(MAX_NAME_BYTES, "field name")?;
            if !field_names.insert(name.clone()) {
                return Err(invalid("DISP Data table contains a duplicate field"));
            }
            let storage = reader.text(MAX_TYPE_BYTES, "field storage type")?;
            let flags = reader.u8("field flags")?;
            if flags & !0b11 != 0 {
                return Err(invalid("DISP Data field uses unknown required flags"));
            }
            let primary = flags & 0b10 != 0;
            primary_count += usize::from(primary);
            fields.push(Field {
                name,
                storage,
                optional: flags & 0b01 != 0,
                primary,
            });
        }
        if primary_count != 1 {
            return Err(invalid(
                "DISP Data table must contain exactly one primary field",
            ));
        }
        let row_count = reader.u64("row count")?;
        if row_count > MAX_ROWS {
            return Err(invalid("DISP Data table exceeds 100000 rows"));
        }
        let mut rows = Vec::with_capacity(row_count);
        for _ in 0..row_count {
            rows.push(reader.data(MAX_ROW_BYTES, "stored row")?);
        }
        tables.push(Table { name, fields, rows });
    }
    if reader.at != payload.len() {
        return Err(invalid(
            "DISP Data snapshot contains trailing payload bytes",
        ));
    }
    Ok(Snapshot { tables })
}

fn suffix(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("DISP Data path has no valid UTF-8 file name"))?;
    Ok(path.with_file_name(format!("{name}{suffix}")))
}

fn read_snapshot(path: &Path) -> io::Result<Snapshot> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(invalid("DISP Data snapshot exceeds 64 MiB"));
    }
    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)?;
    decode(&bytes)
}

pub(crate) fn load(path: &Path) -> io::Result<Snapshot> {
    let backup = suffix(path, ".backup")?;
    match read_snapshot(path) {
        Ok(snapshot) => Ok(snapshot),
        Err(error) if error.kind() == io::ErrorKind::NotFound && backup.is_file() => {
            let snapshot = read_snapshot(&backup).map_err(|backup_error| {
                invalid(format!(
                    "DISP Data snapshot and recovery backup are unavailable: {error}; {backup_error}"
                ))
            })?;
            fs::rename(&backup, path)?;
            Ok(snapshot)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Snapshot::empty()),
        Err(error) => Err(error),
    }
}

pub(crate) fn commit(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let bytes = encode(snapshot)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let id = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = suffix(path, &format!(".tmp-{}-{id}", std::process::id()))?;
    let backup = suffix(path, ".backup")?;

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);

        if backup.exists() {
            fs::remove_file(&backup)?;
        }
        let had_previous = path.exists();
        if had_previous {
            fs::rename(path, &backup)?;
        }
        if let Err(error) = fs::rename(&temporary, path) {
            if had_previous {
                let _ = fs::rename(&backup, path);
            }
            return Err(error);
        }
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        if had_previous {
            // The new main snapshot is already durable. A stale recovery copy is
            // harmless and must not make an otherwise committed mutation appear
            // to have failed to the caller.
            let _ = fs::remove_file(&backup);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            tables: vec![Table {
                name: "User".into(),
                fields: vec![
                    Field {
                        name: "id".into(),
                        storage: "INTEGER".into(),
                        optional: false,
                        primary: true,
                    },
                    Field {
                        name: "name".into(),
                        storage: "TEXT".into(),
                        optional: false,
                        primary: false,
                    },
                ],
                rows: vec![br#"{"id":1,"name":"Ada"}"#.to_vec()],
            }],
        }
    }

    fn temporary(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "disp-data-format-{}-{nonce}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn format_is_deterministic_bounded_and_round_trips() {
        let snapshot = sample();
        let first = encode(&snapshot).unwrap();
        let second = encode(&snapshot).unwrap();
        assert_eq!(first, second);
        assert_eq!(decode(&first).unwrap(), snapshot);

        let mut corrupt = first;
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(
            decode(&corrupt)
                .unwrap_err()
                .to_string()
                .contains("integrity")
        );
        corrupt.pop();
        assert!(decode(&corrupt).is_err());
    }

    #[test]
    fn atomic_commit_reopens_and_recovers_a_completed_backup() {
        let path = temporary("recovery.dispdb");
        commit(&path, &sample()).unwrap();
        assert_eq!(load(&path).unwrap(), sample());

        let backup = suffix(&path, ".backup").unwrap();
        fs::rename(&path, &backup).unwrap();
        assert_eq!(load(&path).unwrap(), sample());
        assert!(path.is_file());
        assert!(!backup.exists());
    }
}
