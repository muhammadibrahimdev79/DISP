use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAGIC: &[u8; 8] = b"DISPDB\x1a\n";
const WAL_MAGIC: &[u8; 8] = b"DISPWAL\n";
const LEGACY_VERSION: u32 = 1;
const PAGE_VERSION: u32 = 2;
const VERSION: u32 = 3;
const LEGACY_HEADER_SIZE: usize = 32;
const PAGE_SIZE: usize = 4096;
const PAGE_HEADER_SIZE: usize = 32;
const PAGE_PAYLOAD_SIZE: usize = PAGE_SIZE - PAGE_HEADER_SIZE;
const WAL_HEADER_SIZE: usize = 64;
const WAL_RECORD_SIZE: usize = 8 + PAGE_SIZE;
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_PAGES: usize = MAX_FILE_BYTES / PAGE_SIZE;
const MAX_WAL_BYTES: usize = WAL_HEADER_SIZE + MAX_PAGES * WAL_RECORD_SIZE;
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
    pub unique: bool,
}

impl Snapshot {
    pub(crate) fn empty() -> Self {
        Self { tables: Vec::new() }
    }
}

#[derive(Debug)]
pub(crate) struct Lock {
    path: PathBuf,
    #[cfg(windows)]
    handle: *mut std::ffi::c_void,
    #[cfg(unix)]
    _file: File,
}

// The operating-system handle is only closed during Drop. The OS owns all
// synchronization state, so moving the guard between runtime threads is safe.
#[cfg(windows)]
unsafe impl Send for Lock {}
#[cfg(windows)]
unsafe impl Sync for Lock {}

#[cfg(windows)]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut std::ffi::c_void,
        creation: u32,
        attributes: u32,
        template: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

#[cfg(unix)]
unsafe extern "C" {
    fn flock(file: std::os::raw::c_int, operation: std::os::raw::c_int) -> std::os::raw::c_int;
}

impl Lock {
    pub(crate) fn acquire(path: &Path) -> io::Result<Self> {
        let path = suffix(path, ".lock")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let wide = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            // SAFETY: `wide` is terminated and remains alive for the call. A zero
            // share mode gives this process exclusive ownership until Drop.
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    0x8000_0000 | 0x4000_0000,
                    0,
                    std::ptr::null_mut(),
                    4,
                    0x80,
                    std::ptr::null_mut(),
                )
            };
            if handle as isize == -1 {
                let cause = io::Error::last_os_error();
                return Err(io::Error::new(
                    cause.kind(),
                    format!("DISP Data store is already open or unavailable: {cause}"),
                ));
            }
            Ok(Self { path, handle })
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)?;
            // SAFETY: the descriptor is live for the call and retained in `Self`.
            if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
                let cause = io::Error::last_os_error();
                return Err(io::Error::new(
                    cause.kind(),
                    format!("DISP Data store is already open or unavailable: {cause}"),
                ));
            }
            Ok(Self { path, _file: file })
        }
    }

    fn protects(&self, path: &Path) -> io::Result<bool> {
        Ok(self.path == suffix(path, ".lock")?)
    }
}

#[cfg(windows)]
impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: this guard exclusively owns the live handle.
        unsafe { CloseHandle(self.handle) };
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

fn encode_payload(snapshot: &Snapshot) -> io::Result<Vec<u8>> {
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
            payload.push(
                u8::from(field.optional)
                    | (u8::from(field.primary) << 1)
                    | (u8::from(field.unique) << 2),
            );
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
        if payload.len() > MAX_FILE_BYTES - PAGE_SIZE {
            return Err(invalid("DISP Data snapshot exceeds 64 MiB"));
        }
    }
    Ok(payload)
}

fn encode_pages(snapshot: &Snapshot, generation: u64) -> io::Result<Vec<u8>> {
    let payload = encode_payload(snapshot)?;
    let data_pages = payload.len().div_ceil(PAGE_PAYLOAD_SIZE).max(1);
    let page_count = data_pages
        .checked_add(1)
        .filter(|count| *count <= MAX_PAGES)
        .ok_or_else(|| invalid("DISP Data page count exceeds its storage limit"))?;
    let mut output = vec![0_u8; page_count * PAGE_SIZE];
    output[..8].copy_from_slice(MAGIC);
    output[8..12].copy_from_slice(&VERSION.to_le_bytes());
    output[12..16].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
    output[16..24].copy_from_slice(&generation.to_le_bytes());
    output[24..32].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    output[32..40].copy_from_slice(&(page_count as u64).to_le_bytes());
    output[40..48].copy_from_slice(&fnv1a(&payload).to_le_bytes());
    output[48..56].copy_from_slice(&0_u64.to_le_bytes());
    let header_checksum = fnv1a(&output[..56]);
    output[56..64].copy_from_slice(&header_checksum.to_le_bytes());

    for page in 0..data_pages {
        let logical_start = page * PAGE_PAYLOAD_SIZE;
        let logical_end = (logical_start + PAGE_PAYLOAD_SIZE).min(payload.len());
        let data = &payload[logical_start..logical_end];
        let start = (page + 1) * PAGE_SIZE;
        output[start] = 1;
        output[start + 4..start + 8].copy_from_slice(&((page + 1) as u32).to_le_bytes());
        output[start + 8..start + 12].copy_from_slice(&(data.len() as u32).to_le_bytes());
        let next = if page + 1 == data_pages {
            0
        } else {
            (page + 2) as u32
        };
        output[start + 12..start + 16].copy_from_slice(&next.to_le_bytes());
        output[start + 16..start + 24].copy_from_slice(&fnv1a(data).to_le_bytes());
        output[start + PAGE_HEADER_SIZE..start + PAGE_HEADER_SIZE + data.len()]
            .copy_from_slice(data);
    }
    Ok(output)
}

#[cfg(test)]
fn encode(snapshot: &Snapshot) -> io::Result<Vec<u8>> {
    encode_pages(snapshot, 1)
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

fn decode_payload(payload: &[u8], supports_unique: bool) -> io::Result<Snapshot> {
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
            let allowed = if supports_unique { 0b111 } else { 0b11 };
            if flags & !allowed != 0 {
                return Err(invalid("DISP Data field uses unknown required flags"));
            }
            let primary = flags & 0b10 != 0;
            primary_count += usize::from(primary);
            fields.push(Field {
                name,
                storage,
                optional: flags & 0b01 != 0,
                primary,
                unique: flags & 0b100 != 0,
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

fn read_u32(bytes: &[u8], start: usize) -> u32 {
    u32::from_le_bytes(bytes[start..start + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], start: usize) -> u64 {
    u64::from_le_bytes(bytes[start..start + 8].try_into().unwrap())
}

fn decode_legacy(bytes: &[u8]) -> io::Result<Snapshot> {
    if bytes.len() < LEGACY_HEADER_SIZE {
        return Err(invalid("DISP Data snapshot header is truncated"));
    }
    if read_u32(bytes, 12) != 0 {
        return Err(invalid("DISP Data snapshot uses unknown required flags"));
    }
    let payload_length = usize::try_from(read_u64(bytes, 16))
        .map_err(|_| invalid("DISP Data snapshot length does not fit this target"))?;
    if payload_length != bytes.len() - LEGACY_HEADER_SIZE {
        return Err(invalid("DISP Data snapshot length is inconsistent"));
    }
    let payload = &bytes[LEGACY_HEADER_SIZE..];
    if fnv1a(payload) != read_u64(bytes, 24) {
        return Err(invalid("DISP Data snapshot integrity check failed"));
    }
    decode_payload(payload, false)
}

fn decode_pages(bytes: &[u8], supports_unique: bool) -> io::Result<Snapshot> {
    if bytes.len() < PAGE_SIZE {
        return Err(invalid("DISP Data page header is truncated"));
    }
    if read_u32(bytes, 12) as usize != PAGE_SIZE {
        return Err(invalid("DISP Data snapshot uses an unsupported page size"));
    }
    if read_u64(bytes, 48) != 0 {
        return Err(invalid("DISP Data snapshot uses unknown required flags"));
    }
    if fnv1a(&bytes[..56]) != read_u64(bytes, 56) {
        return Err(invalid("DISP Data page header integrity check failed"));
    }
    if bytes[64..PAGE_SIZE].iter().any(|byte| *byte != 0) {
        return Err(invalid("DISP Data page header contains unknown metadata"));
    }
    let payload_length = usize::try_from(read_u64(bytes, 24))
        .map_err(|_| invalid("DISP Data payload length does not fit this target"))?;
    let page_count = usize::try_from(read_u64(bytes, 32))
        .map_err(|_| invalid("DISP Data page count does not fit this target"))?;
    if !(2..=MAX_PAGES).contains(&page_count) || bytes.len() != page_count * PAGE_SIZE {
        return Err(invalid("DISP Data page count is inconsistent"));
    }
    if payload_length > (page_count - 1) * PAGE_PAYLOAD_SIZE {
        return Err(invalid("DISP Data payload exceeds its page chain"));
    }
    let mut payload = Vec::with_capacity(payload_length);
    for page in 1..page_count {
        let start = page * PAGE_SIZE;
        if bytes[start] != 1 || bytes[start + 1..start + 4] != [0, 0, 0] {
            return Err(invalid("DISP Data page has an invalid type or flags"));
        }
        if read_u32(bytes, start + 4) as usize != page {
            return Err(invalid("DISP Data page identity is inconsistent"));
        }
        let used = read_u32(bytes, start + 8) as usize;
        let remaining = payload_length - payload.len();
        let expected_used = remaining.min(PAGE_PAYLOAD_SIZE);
        if used != expected_used {
            return Err(invalid("DISP Data page used length is inconsistent"));
        }
        let expected_next = if page + 1 == page_count {
            0
        } else {
            (page + 1) as u32
        };
        if read_u32(bytes, start + 12) != expected_next || read_u64(bytes, start + 24) != 0 {
            return Err(invalid("DISP Data page chain is inconsistent"));
        }
        let data = &bytes[start + PAGE_HEADER_SIZE..start + PAGE_HEADER_SIZE + used];
        if fnv1a(data) != read_u64(bytes, start + 16) {
            return Err(invalid("DISP Data page integrity check failed"));
        }
        if bytes[start + PAGE_HEADER_SIZE + used..start + PAGE_SIZE]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(invalid("DISP Data page contains non-zero unused bytes"));
        }
        payload.extend_from_slice(data);
    }
    if fnv1a(&payload) != read_u64(bytes, 40) {
        return Err(invalid("DISP Data payload integrity check failed"));
    }
    decode_payload(&payload, supports_unique)
}

pub(crate) fn decode(bytes: &[u8]) -> io::Result<Snapshot> {
    if bytes.len() < 12 {
        return Err(invalid("DISP Data snapshot header is truncated"));
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(invalid("DISP Data snapshot exceeds 64 MiB"));
    }
    if &bytes[..8] != MAGIC {
        return Err(invalid("file is not a DISP Data snapshot"));
    }
    match read_u32(bytes, 8) {
        LEGACY_VERSION => decode_legacy(bytes),
        PAGE_VERSION => decode_pages(bytes, false),
        VERSION => decode_pages(bytes, true),
        version => Err(invalid(format!(
            "unsupported DISP Data snapshot version {version}"
        ))),
    }
}

fn suffix(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("DISP Data path has no valid UTF-8 file name"))?;
    Ok(path.with_file_name(format!("{name}{suffix}")))
}

fn read_bytes(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > limit as u64 {
        return Err(invalid("DISP Data file exceeds its storage limit"));
    }
    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_snapshot(path: &Path) -> io::Result<Snapshot> {
    decode(&read_bytes(path, MAX_FILE_BYTES)?)
}

fn wal_records<'a>(old: &[u8], new: &'a [u8]) -> Vec<(u64, &'a [u8])> {
    new.chunks_exact(PAGE_SIZE)
        .enumerate()
        .filter_map(|(page, bytes)| {
            let old_start = page * PAGE_SIZE;
            (old.get(old_start..old_start + PAGE_SIZE) != Some(bytes))
                .then_some((page as u64, bytes))
        })
        .collect()
}

fn encode_wal(old: &[u8], new: &[u8], generation: u64) -> io::Result<Vec<u8>> {
    let records = wal_records(old, new);
    let length = WAL_HEADER_SIZE
        .checked_add(records.len() * WAL_RECORD_SIZE)
        .filter(|length| *length <= MAX_WAL_BYTES)
        .ok_or_else(|| invalid("DISP Data write-ahead log exceeds its storage limit"))?;
    let mut output = Vec::with_capacity(length);
    output.extend_from_slice(WAL_MAGIC);
    output.extend_from_slice(&1_u32.to_le_bytes());
    output.extend_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
    output.extend_from_slice(&generation.to_le_bytes());
    output.extend_from_slice(&((new.len() / PAGE_SIZE) as u64).to_le_bytes());
    output.extend_from_slice(&(records.len() as u64).to_le_bytes());
    output.extend_from_slice(&0_u64.to_le_bytes());
    output.extend_from_slice(&0_u64.to_le_bytes());
    output.extend_from_slice(&0_u64.to_le_bytes());
    for (page, bytes) in records {
        output.extend_from_slice(&page.to_le_bytes());
        output.extend_from_slice(bytes);
    }
    let checksum = fnv1a(&output[WAL_HEADER_SIZE..]);
    output[40..48].copy_from_slice(&checksum.to_le_bytes());
    let header_checksum = fnv1a(&output[..56]);
    output[56..64].copy_from_slice(&header_checksum.to_le_bytes());
    Ok(output)
}

fn apply_wal(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() < WAL_HEADER_SIZE || bytes.len() > MAX_WAL_BYTES {
        return Err(invalid("DISP Data write-ahead log header is truncated"));
    }
    if &bytes[..8] != WAL_MAGIC
        || read_u32(bytes, 8) != 1
        || read_u32(bytes, 12) as usize != PAGE_SIZE
        || read_u64(bytes, 48) != 0
        || fnv1a(&bytes[..56]) != read_u64(bytes, 56)
    {
        return Err(invalid("DISP Data write-ahead log header is invalid"));
    }
    let page_count = usize::try_from(read_u64(bytes, 24))
        .map_err(|_| invalid("DISP Data WAL page count does not fit this target"))?;
    let record_count = usize::try_from(read_u64(bytes, 32))
        .map_err(|_| invalid("DISP Data WAL record count does not fit this target"))?;
    let expected_length = WAL_HEADER_SIZE
        .checked_add(record_count.saturating_mul(WAL_RECORD_SIZE))
        .ok_or_else(|| invalid("DISP Data WAL length overflow"))?;
    if !(2..=MAX_PAGES).contains(&page_count)
        || expected_length != bytes.len()
        || fnv1a(&bytes[WAL_HEADER_SIZE..]) != read_u64(bytes, 40)
    {
        return Err(invalid("DISP Data write-ahead log is inconsistent"));
    }
    let mut seen = HashSet::new();
    let mut records = Vec::with_capacity(record_count);
    for record in 0..record_count {
        let start = WAL_HEADER_SIZE + record * WAL_RECORD_SIZE;
        let page = usize::try_from(read_u64(bytes, start))
            .map_err(|_| invalid("DISP Data WAL page identity does not fit this target"))?;
        if page >= page_count || !seen.insert(page) {
            return Err(invalid("DISP Data WAL contains an invalid page identity"));
        }
        records.push((page, &bytes[start + 8..start + WAL_RECORD_SIZE]));
    }
    if !seen.contains(&0) {
        return Err(invalid("DISP Data WAL does not contain its commit page"));
    }

    let final_length = page_count * PAGE_SIZE;
    let mut committed = vec![0_u8; final_length];
    match read_bytes(path, MAX_FILE_BYTES) {
        Ok(current) => {
            let copied = current.len().min(final_length);
            committed[..copied].copy_from_slice(&current[..copied]);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    for (page, data) in &records {
        let start = page * PAGE_SIZE;
        committed[start..start + PAGE_SIZE].copy_from_slice(data);
    }
    if current_generation(&committed) != read_u64(bytes, 16) {
        return Err(invalid(
            "DISP Data WAL generation does not match its commit page",
        ));
    }
    decode(&committed)
        .map_err(|error| invalid(format!("DISP Data WAL committed image is invalid: {error}")))?;

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    for (page, data) in records {
        file.seek(SeekFrom::Start((page * PAGE_SIZE) as u64))?;
        file.write_all(data)?;
    }
    file.set_len(final_length as u64)?;
    file.sync_all()
}

fn recover_wal(path: &Path) -> io::Result<()> {
    let wal = suffix(path, ".wal")?;
    match read_bytes(&wal, MAX_WAL_BYTES) {
        Ok(bytes) => {
            apply_wal(path, &bytes)?;
            fs::remove_file(wal)?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn load(path: &Path, lock: &Lock) -> io::Result<Snapshot> {
    if !lock.protects(path)? {
        return Err(invalid("DISP Data lock does not protect this store"));
    }
    recover_wal(path)?;
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

fn current_generation(bytes: &[u8]) -> u64 {
    if bytes.len() >= PAGE_SIZE && &bytes[..8] == MAGIC && read_u32(bytes, 8) == VERSION {
        read_u64(bytes, 16)
    } else {
        0
    }
}

pub(crate) fn commit(path: &Path, snapshot: &Snapshot, lock: &Lock) -> io::Result<()> {
    if !lock.protects(path)? {
        return Err(invalid("DISP Data lock does not protect this store"));
    }
    recover_wal(path)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let old = match read_bytes(path, MAX_FILE_BYTES) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let generation = current_generation(&old)
        .checked_add(1)
        .ok_or_else(|| invalid("DISP Data generation counter is exhausted"))?;
    let new = encode_pages(snapshot, generation)?;
    let wal_bytes = encode_wal(&old, &new, generation)?;
    let id = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = suffix(path, &format!(".wal.tmp-{}-{id}", std::process::id()))?;
    let wal = suffix(path, ".wal")?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&wal_bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &wal)?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        // Renaming the synced WAL is the commit point. Checkpoint failure after
        // this point cannot be reported as an aborted transaction: recovery will
        // roll the committed pages forward on the next access.
        if apply_wal(path, &wal_bytes).is_ok() {
            fs::remove_file(&wal)?;
            let _ = fs::remove_file(suffix(path, ".backup")?);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_with_note(note: &[u8]) -> Snapshot {
        Snapshot {
            tables: vec![Table {
                name: "User".into(),
                fields: vec![
                    Field {
                        name: "id".into(),
                        storage: "INTEGER".into(),
                        optional: false,
                        primary: true,
                        unique: false,
                    },
                    Field {
                        name: "name".into(),
                        storage: "TEXT".into(),
                        optional: false,
                        primary: false,
                        unique: false,
                    },
                ],
                rows: vec![note.to_vec()],
            }],
        }
    }

    fn sample() -> Snapshot {
        sample_with_note(br#"{"id":1,"name":"Ada"}"#)
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

    fn encode_legacy(snapshot: &Snapshot) -> Vec<u8> {
        let payload = encode_payload(snapshot).unwrap();
        let mut output = Vec::new();
        output.extend_from_slice(MAGIC);
        output.extend_from_slice(&LEGACY_VERSION.to_le_bytes());
        output.extend_from_slice(&0_u32.to_le_bytes());
        output.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        output.extend_from_slice(&fnv1a(&payload).to_le_bytes());
        output.extend_from_slice(&payload);
        output
    }

    #[test]
    fn fixed_pages_are_deterministic_bounded_and_read_legacy_snapshots() {
        let snapshot = sample();
        let first = encode(&snapshot).unwrap();
        let second = encode(&snapshot).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len() % PAGE_SIZE, 0);
        assert_eq!(read_u32(&first, 8), VERSION);
        assert_eq!(decode(&first).unwrap(), snapshot);
        assert_eq!(decode(&encode_legacy(&snapshot)).unwrap(), snapshot);

        let mut version_two = first.clone();
        version_two[8..12].copy_from_slice(&PAGE_VERSION.to_le_bytes());
        let header_checksum = fnv1a(&version_two[..56]);
        version_two[56..64].copy_from_slice(&header_checksum.to_le_bytes());
        assert_eq!(decode(&version_two).unwrap(), snapshot);

        let mut corrupt = first;
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(decode(&corrupt).is_err());
        corrupt.pop();
        assert!(decode(&corrupt).is_err());
    }

    #[test]
    fn wal_contains_only_changed_pages_and_recovers_idempotently() {
        let large = vec![b'x'; PAGE_PAYLOAD_SIZE * 3];
        let old_snapshot = sample_with_note(&large);
        let mut changed = large;
        *changed.last_mut().unwrap() = b'y';
        let new_snapshot = sample_with_note(&changed);
        let old = encode_pages(&old_snapshot, 1).unwrap();
        let new = encode_pages(&new_snapshot, 2).unwrap();
        let wal = encode_wal(&old, &new, 2).unwrap();
        assert_eq!(read_u64(&wal, 32), 2, "header plus one data page");

        let path = temporary("wal-recovery.dispdb");
        fs::write(&path, &old).unwrap();
        apply_wal(&path, &wal).unwrap();
        apply_wal(&path, &wal).unwrap();
        assert_eq!(decode(&fs::read(path).unwrap()).unwrap(), new_snapshot);
    }

    #[test]
    fn commit_reopens_recovers_a_committed_wal_and_migrates_v1() {
        let path = temporary("recovery.dispdb");
        let lock = Lock::acquire(&path).unwrap();
        fs::write(&path, encode_legacy(&sample())).unwrap();
        assert_eq!(load(&path, &lock).unwrap(), sample());
        commit(&path, &sample(), &lock).unwrap();
        let migrated = fs::read(&path).unwrap();
        assert_eq!(read_u32(&migrated, 8), VERSION);

        let updated = sample_with_note(br#"{"id":1,"name":"Grace"}"#);
        let next = encode_pages(&updated, 2).unwrap();
        let wal = encode_wal(&migrated, &next, 2).unwrap();
        fs::write(suffix(&path, ".wal").unwrap(), wal).unwrap();
        assert_eq!(load(&path, &lock).unwrap(), updated);
        assert!(!suffix(&path, ".wal").unwrap().exists());
    }

    #[test]
    fn a_second_process_guard_is_rejected_while_the_store_is_open() {
        let path = temporary("locking.dispdb");
        let first = Lock::acquire(&path).unwrap();
        assert!(Lock::acquire(&path).is_err());
        drop(first);
        assert!(Lock::acquire(&path).is_ok());
    }
}
