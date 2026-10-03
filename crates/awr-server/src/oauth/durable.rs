//! Encrypted, atomic snapshots for one coordinator process. PostgreSQL remains
//! the live authority; this file only preserves approved OAuth connections.
use super::*;
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"AWR-OAUTH\x01";
const MAX_BYTES: u64 = 64 * 1024 * 1024;
type Key = [u8; 32];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u8,
    binding: Key,
    resources: BTreeSet<String>,
    saved_at_ns: u64,
    clients: Vec<(String, SavedClient)>,
    grants: Vec<(Key, SavedGrant)>,
    tokens: Vec<(Key, SavedToken)>,
    refresh_tokens: Vec<(Key, RefreshToken)>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedClient {
    value: RegisteredClient,
    expires_ns: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedGrant {
    client_id: String,
    resource: String,
    bearer: String,
    expires_ns: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedToken {
    family: Key,
    expires_ns: u64,
}

struct Clock {
    monotonic: Instant,
    wall_ns: u64,
}
fn wall_ns() -> Result<u64, OAuthError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_nanos()).ok())
        .ok_or(OAuthError::Unavailable)
}
impl Clock {
    fn new() -> Result<Self, OAuthError> {
        Ok(Self {
            monotonic: Instant::now(),
            wall_ns: wall_ns()?,
        })
    }
    fn deadline(&self, expires: Instant) -> Result<u64, OAuthError> {
        let duration = expires.saturating_duration_since(self.monotonic);
        self.wall_ns
            .checked_add(u64::try_from(duration.as_nanos()).map_err(|_| OAuthError::Unavailable)?)
            .ok_or(OAuthError::Unavailable)
    }
    fn restore(&self, expires_ns: u64) -> Result<Instant, OAuthError> {
        self.monotonic
            .checked_add(Duration::from_nanos(
                expires_ns.saturating_sub(self.wall_ns),
            ))
            .ok_or(OAuthError::Unavailable)
    }
}

pub(super) struct Store {
    directory: PathBuf,
    key: LessSafeKey,
    binding: Key,
    clock: Clock,
    failed: AtomicBool,
    last_saved_at_ns: AtomicU64,
    // Keep the exclusive file lock for the whole store lifetime.
    _lock: File,
}

// Unix private files are checked both before opening and on the opened handle.
// Refuse links, other owners and permissive modes instead of silently chmodding.
#[cfg(unix)]
fn private_metadata(metadata: &fs::Metadata, directory: bool) -> Result<(), OAuthError> {
    use std::os::unix::fs::MetadataExt;
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != if directory { 0o700 } else { 0o600 }
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file() || metadata.nlink() != 1
        }
    {
        return Err(OAuthError::Unavailable);
    }
    Ok(())
}
#[cfg(not(unix))]
fn private_metadata(_: &fs::Metadata, _: bool) -> Result<(), OAuthError> {
    // Do not claim equivalent permission/durability guarantees on other hosts.
    Err(OAuthError::Unavailable)
}
fn open_file(path: &Path, create: bool) -> Result<File, OAuthError> {
    let mut options = OpenOptions::new();
    options.read(true).write(create);
    if create {
        options.create_new(true);
    } else {
        private_metadata(
            &fs::symlink_metadata(path).map_err(|_| OAuthError::Unavailable)?,
            false,
        )?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| OAuthError::Unavailable)?;
    private_metadata(
        &file.metadata().map_err(|_| OAuthError::Unavailable)?,
        false,
    )?;
    Ok(file)
}
fn exists(path: &Path) -> Result<bool, OAuthError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(OAuthError::Unavailable),
    }
}
fn read_file(path: &Path, limit: u64) -> Result<Zeroizing<Vec<u8>>, OAuthError> {
    let file = open_file(path, false)?;
    if file.metadata().map_err(|_| OAuthError::Unavailable)?.len() > limit {
        return Err(OAuthError::Unavailable);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| OAuthError::Unavailable)?;
    if bytes.len() as u64 > limit {
        return Err(OAuthError::Unavailable);
    }
    Ok(bytes)
}
fn sync_directory(path: &Path) -> Result<(), OAuthError> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|_| OAuthError::Unavailable)
}

impl Store {
    pub(super) fn open(
        directory: &Path,
        resources: &BTreeSet<String>,
        binding: &str,
    ) -> Result<(Self, Entries), OAuthError> {
        if !cfg!(unix) || !directory.is_absolute() || !bounded(binding, 65536) {
            return Err(OAuthError::Unavailable);
        }
        let fresh = !exists(directory)?;
        if fresh {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder
                .create(directory)
                .map_err(|_| OAuthError::Unavailable)?;
        }
        private_metadata(
            &fs::symlink_metadata(directory).map_err(|_| OAuthError::Unavailable)?,
            true,
        )?;
        let lock_path = directory.join("writer.lock");
        let lock = open_file(&lock_path, !exists(&lock_path)?)?;
        lock.try_lock().map_err(|_| OAuthError::Unavailable)?;
        let key_path = directory.join("key.bin");
        let state_path = directory.join("state.bin");
        // A partial/missing pair is never interpreted as a new empty store.
        if !fresh && (!exists(&key_path)? || !exists(&state_path)?) {
            return Err(OAuthError::Unavailable);
        }
        let mut key_bytes = Zeroizing::new([0u8; 32]);
        if fresh {
            getrandom::fill(key_bytes.as_mut()).map_err(|_| OAuthError::Unavailable)?;
            let mut file = open_file(&key_path, true)?;
            file.write_all(key_bytes.as_ref())
                .and_then(|_| file.sync_all())
                .map_err(|_| OAuthError::Unavailable)?;
        } else {
            let bytes = read_file(&key_path, 32)?;
            if bytes.len() != 32 {
                return Err(OAuthError::Unavailable);
            }
            key_bytes.copy_from_slice(&bytes);
        }
        let store = Self {
            directory: directory.to_owned(),
            key: LessSafeKey::new(
                UnboundKey::new(&aead::AES_256_GCM, key_bytes.as_ref())
                    .map_err(|_| OAuthError::Unavailable)?,
            ),
            binding: hash(binding),
            clock: Clock::new()?,
            failed: AtomicBool::new(false),
            last_saved_at_ns: AtomicU64::new(0),
            _lock: lock,
        };
        let entries = if fresh {
            let entries = Entries::default();
            store.save(&entries, resources)?;
            sync_directory(directory.parent().ok_or(OAuthError::Unavailable)?)?;
            entries
        } else {
            store.load(resources)?
        };
        // A crash before rename can leave only an encrypted next snapshot.
        // The committed state is authoritative. Remove the orphan under lock.
        let next = directory.join("state.next");
        if exists(&next)? {
            private_metadata(
                &fs::symlink_metadata(&next).map_err(|_| OAuthError::Unavailable)?,
                false,
            )?;
            fs::remove_file(next).map_err(|_| OAuthError::Unavailable)?;
            sync_directory(directory)?;
        }
        Ok((store, entries))
    }

    pub(super) fn check(&self) -> Result<(), OAuthError> {
        if self.failed.load(Ordering::Acquire) {
            Err(OAuthError::Unavailable)
        } else {
            Ok(())
        }
    }

    pub(super) fn save(
        &self,
        entries: &Entries,
        resources: &BTreeSet<String>,
    ) -> Result<(), OAuthError> {
        self.check()?;
        let result = self.write_snapshot(entries, resources);
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
        }
        result
    }

    fn write_snapshot(
        &self,
        entries: &Entries,
        resources: &BTreeSet<String>,
    ) -> Result<(), OAuthError> {
        private_metadata(
            &fs::symlink_metadata(&self.directory).map_err(|_| OAuthError::Unavailable)?,
            true,
        )?;
        let saved_at_ns = wall_ns()?;
        if saved_at_ns < self.last_saved_at_ns.load(Ordering::Acquire) {
            return Err(OAuthError::Unavailable);
        }
        let snapshot = Snapshot {
            version: 1,
            binding: self.binding,
            resources: resources.clone(),
            saved_at_ns,
            clients: entries
                .clients
                .iter()
                .map(|(k, e)| {
                    Ok((
                        k.clone(),
                        SavedClient {
                            value: e.value.clone(),
                            expires_ns: self.clock.deadline(e.expires)?,
                        },
                    ))
                })
                .collect::<Result<_, OAuthError>>()?,
            grants: entries
                .grants
                .iter()
                .map(|(k, e)| {
                    Ok((
                        *k,
                        SavedGrant {
                            client_id: e.client_id.clone(),
                            resource: e.resource.clone(),
                            bearer: e.bearer.clone(),
                            expires_ns: self.clock.deadline(e.expires)?,
                        },
                    ))
                })
                .collect::<Result<_, OAuthError>>()?,
            tokens: entries
                .tokens
                .iter()
                .map(|(k, e)| {
                    Ok((
                        *k,
                        SavedToken {
                            family: e.family,
                            expires_ns: self.clock.deadline(e.expires)?,
                        },
                    ))
                })
                .collect::<Result<_, OAuthError>>()?,
            refresh_tokens: entries
                .refresh_tokens
                .iter()
                .map(|(k, e)| (*k, e.clone()))
                .collect(),
        };
        let mut payload =
            Zeroizing::new(serde_json::to_vec(&snapshot).map_err(|_| OAuthError::Unavailable)?);
        if payload.len() as u64 + 64 > MAX_BYTES {
            return Err(OAuthError::Unavailable);
        }
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).map_err(|_| OAuthError::Unavailable)?;
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(MAGIC),
                &mut *payload,
            )
            .map_err(|_| OAuthError::Unavailable)?;
        let path = self.directory.join("state.bin");
        if exists(&path)? {
            private_metadata(
                &fs::symlink_metadata(&path).map_err(|_| OAuthError::Unavailable)?,
                false,
            )?;
        }
        let next = self.directory.join("state.next");
        let mut file = open_file(&next, true)?;
        file.write_all(MAGIC)
            .and_then(|_| file.write_all(&nonce))
            .and_then(|_| file.write_all(&payload))
            .and_then(|_| file.sync_all())
            .map_err(|_| OAuthError::Unavailable)?;
        fs::rename(next, path).map_err(|_| OAuthError::Unavailable)?;
        sync_directory(&self.directory)?;
        self.last_saved_at_ns.store(saved_at_ns, Ordering::Release);
        Ok(())
    }

    fn load(&self, resources: &BTreeSet<String>) -> Result<Entries, OAuthError> {
        let bytes = read_file(&self.directory.join("state.bin"), MAX_BYTES)?;
        if bytes.len() < MAGIC.len() + 12 + aead::AES_256_GCM.tag_len() || !bytes.starts_with(MAGIC)
        {
            return Err(OAuthError::Unavailable);
        }
        let nonce: [u8; 12] = bytes[MAGIC.len()..MAGIC.len() + 12]
            .try_into()
            .map_err(|_| OAuthError::Unavailable)?;
        let mut payload = Zeroizing::new(bytes[MAGIC.len() + 12..].to_vec());
        let plaintext = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(MAGIC),
                &mut payload,
            )
            .map_err(|_| OAuthError::Unavailable)?;
        let snapshot: Snapshot =
            serde_json::from_slice(plaintext).map_err(|_| OAuthError::Unavailable)?;
        if snapshot.version != 1
            || snapshot.binding != self.binding
            || &snapshot.resources != resources
            || snapshot.saved_at_ns > self.clock.wall_ns
            || snapshot.clients.len() > MAX_CLIENTS
            || snapshot.grants.len() > MAX_TOKENS
            || snapshot.tokens.len() > MAX_TOKENS
            || snapshot.refresh_tokens.len() > MAX_REFRESH_TOKENS
        {
            return Err(OAuthError::Unavailable);
        }
        self.last_saved_at_ns
            .store(snapshot.saved_at_ns, Ordering::Release);
        let mut entries = Entries::default();
        for (id, e) in snapshot.clients {
            if id != e.value.client_id
                || !bounded(&id, 128)
                || !bounded(&e.value.client_name, 120)
                || e.value.redirect_uris.is_empty()
                || e.value.redirect_uris.len() > 8
                || e.value.redirect_uris.iter().any(|u| !valid_redirect(u))
                || entries
                    .clients
                    .insert(
                        id,
                        Client {
                            value: e.value,
                            expires: self.clock.restore(e.expires_ns)?,
                        },
                    )
                    .is_some()
            {
                return Err(OAuthError::Unavailable);
            }
        }
        for (id, e) in snapshot.grants {
            let expires = self.clock.restore(e.expires_ns)?;
            if !resources.contains(&e.resource)
                || !bounded(&e.bearer, 4096)
                || entries
                    .clients
                    .get(&e.client_id)
                    .is_none_or(|c| expires > c.expires)
                || entries
                    .grants
                    .insert(
                        id,
                        Grant {
                            client_id: e.client_id,
                            resource: e.resource,
                            bearer: e.bearer,
                            expires,
                        },
                    )
                    .is_some()
            {
                return Err(OAuthError::Unavailable);
            }
        }
        for (id, e) in snapshot.tokens {
            let expires = self.clock.restore(e.expires_ns)?;
            if entries
                .grants
                .get(&e.family)
                .is_none_or(|g| expires > g.expires)
                || entries
                    .tokens
                    .insert(
                        id,
                        Token {
                            family: e.family,
                            expires,
                        },
                    )
                    .is_some()
            {
                return Err(OAuthError::Unavailable);
            }
        }
        for (id, e) in snapshot.refresh_tokens {
            if !entries.grants.contains_key(&e.family)
                || entries.refresh_tokens.insert(id, e).is_some()
            {
                return Err(OAuthError::Unavailable);
            }
        }
        entries.prune(self.clock.monotonic);
        Ok(entries)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn clock_rollback_is_rejected_before_restore_or_commit() {
        let directory = std::env::temp_dir().join(random("awr-clock-").unwrap());
        let resources = BTreeSet::from(["https://awr.example/v1/projects/one/mcp".into()]);
        let (mut store, entries) = Store::open(&directory, &resources, "fixture").unwrap();
        let wall = store.clock.wall_ns;
        store.clock.wall_ns = 0;
        assert!(store.load(&resources).is_err());
        store.clock.wall_ns = wall;
        store.last_saved_at_ns.store(u64::MAX, Ordering::Release);
        assert!(store.save(&entries, &resources).is_err());
        assert!(store.check().is_err());
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }
}
