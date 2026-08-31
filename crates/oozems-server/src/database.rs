use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;

use oozems_proto::v1::PlayerState;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use rusqlite::OptionalExtension;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use self::player_record::durable_from_player;
use self::player_record::player_from_durable;

#[path = "database/player_loader.rs"]
mod player_loader;
#[path = "database/player_record.rs"]
mod player_record;
#[path = "database/player_repository.rs"]
mod player_repository;

#[cfg(test)]
#[path = "database/tests.rs"]
mod tests;

const APPLICATION_ID: i32 = 1_330_596_421;
const SCHEMA_VERSION: i32 = 2;
const MINIMUM_SQLITE_VERSION: i32 = 3_051_003;
const READ_WORKER_COUNT: usize = 4;
const COMMAND_QUEUE_CAPACITY: usize = 64;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATION_V1: &str = include_str!("../migrations/0001_normalized_players.sql");
const MIGRATION_V2: &str = include_str!("../migrations/0002_quest_journal_key_action.sql");

type WorkerJob = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

enum WorkerCommand {
    Execute(WorkerJob),
    Shutdown(Option<oneshot::Sender<()>>),
}

struct Worker {
    sender: mpsc::Sender<WorkerCommand>,
}

struct DatabaseInner {
    writer: Worker,
    readers: Vec<Worker>,
    next_reader: AtomicUsize,
    closed: AtomicBool,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl Drop for DatabaseInner {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        let _ = self.writer.sender.try_send(WorkerCommand::Shutdown(None));
        for reader in &self.readers {
            let _ = reader.sender.try_send(WorkerCommand::Shutdown(None));
        }
    }
}

#[derive(Clone)]
pub struct Database {
    inner: Arc<DatabaseInner>,
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("SQLite storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("database worker error: {message}")]
    Worker { message: String },
    #[error("database schema error: {message}")]
    Schema { message: String },
    #[error("invalid player state: {message}")]
    Invalid { message: String },
    #[error("persisted player {player_id:?} is corrupt: {message}")]
    Corrupt { player_id: String, message: String },
    #[error("player {player_id:?} already exists")]
    Exists { player_id: String },
    #[error("player {player_id:?} was not found")]
    NotFound { player_id: String },
    #[error("player {player_id:?} revision conflict: expected {expected}, found {actual}")]
    RevisionConflict {
        player_id: String,
        expected: u64,
        actual: u64,
    },
    #[error("{field} exceeds SQLite's signed integer range")]
    Overflow { field: &'static str },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayerId(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterName(String);

#[derive(Debug, Error)]
pub enum PlayerIdError {
    #[error("player ID must contain 1 to 32 ASCII letters, digits, underscores, or hyphens")]
    Invalid,
}

#[derive(Debug, Error)]
pub enum CharacterNameError {
    #[error("character name must contain 3 to 12 ASCII letters, digits, or underscores")]
    Invalid,
}

impl PlayerId {
    pub fn parse(value: &str) -> Result<Self, PlayerIdError> {
        let is_valid = !value.is_empty()
            && value.len() <= 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        is_valid
            .then(|| Self(value.to_owned()))
            .ok_or(PlayerIdError::Invalid)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CharacterName {
    pub fn parse(value: &str) -> Result<Self, CharacterNameError> {
        let is_valid = (3..=12).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        is_valid
            .then(|| Self(value.to_owned()))
            .ok_or(CharacterNameError::Invalid)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Database {
    pub async fn close(&self) -> Result<(), DatabaseError> {
        if self.inner.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        let mut acknowledgements = Vec::with_capacity(READ_WORKER_COUNT + 1);
        for worker in std::iter::once(&self.inner.writer).chain(&self.inner.readers) {
            let (sender, receiver) = oneshot::channel();
            worker
                .sender
                .send(WorkerCommand::Shutdown(Some(sender)))
                .await
                .map_err(|_| DatabaseError::Worker {
                    message: "worker stopped before shutdown".to_owned(),
                })?;
            acknowledgements.push(receiver);
        }
        for acknowledgement in acknowledgements {
            acknowledgement.await.map_err(|_| DatabaseError::Worker {
                message: "worker stopped without acknowledging shutdown".to_owned(),
            })?;
        }

        let handles =
            std::mem::take(
                &mut *self
                    .inner
                    .handles
                    .lock()
                    .map_err(|_| DatabaseError::Worker {
                        message: "worker handle lock was poisoned".to_owned(),
                    })?,
            );
        for handle in handles {
            handle.join().map_err(|panic| DatabaseError::Worker {
                message: format!("worker panicked: {}", panic_message(panic)),
            })?;
        }
        Ok(())
    }

    async fn read<T, F>(
        &self,
        operation: F,
    ) -> Result<T, DatabaseError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DatabaseError> + Send + 'static,
    {
        let index =
            self.inner.next_reader.fetch_add(1, Ordering::Relaxed) % self.inner.readers.len();
        self.execute(&self.inner.readers[index], operation).await
    }

    async fn write<T, F>(
        &self,
        operation: F,
    ) -> Result<T, DatabaseError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DatabaseError> + Send + 'static,
    {
        self.execute(&self.inner.writer, operation).await
    }

    async fn execute<T, F>(
        &self,
        worker: &Worker,
        operation: F,
    ) -> Result<T, DatabaseError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DatabaseError> + Send + 'static,
    {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(DatabaseError::Worker {
                message: "database is closed".to_owned(),
            });
        }
        let (response_sender, response_receiver) = oneshot::channel();
        worker
            .sender
            .send(WorkerCommand::Execute(Box::new(move |connection| {
                let _ = response_sender.send(operation(connection));
            })))
            .await
            .map_err(|_| DatabaseError::Worker {
                message: "worker command queue is closed".to_owned(),
            })?;
        response_receiver.await.map_err(|_| DatabaseError::Worker {
            message: "worker stopped before returning a response".to_owned(),
        })?
    }
}

pub fn open_sqlite(path: &Path) -> Result<Database, DatabaseError> {
    let (writer, writer_handle) = spawn_worker(path.to_owned(), WorkerKind::Writer)?;
    let mut readers = Vec::with_capacity(READ_WORKER_COUNT);
    let mut handles = vec![writer_handle];
    for index in 0..READ_WORKER_COUNT {
        match spawn_worker(path.to_owned(), WorkerKind::Reader(index)) {
            Ok((reader, handle)) => {
                readers.push(reader);
                handles.push(handle);
            }
            Err(error) => {
                stop_workers(&writer, &readers, handles);
                return Err(error);
            }
        }
    }
    Ok(Database {
        inner: Arc::new(DatabaseInner {
            writer,
            readers,
            next_reader: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            handles: Mutex::new(handles),
        }),
    })
}

pub async fn load_player(
    database: &Database,
    player_id: &PlayerId,
) -> Result<Option<PlayerState>, DatabaseError> {
    let player_id = player_id.as_str().to_owned();
    database
        .read(move |connection| {
            player_loader::load_player(connection, &player_id)?
                .map(|player| player_from_durable(player, None))
                .transpose()
        })
        .await
}

pub async fn create_player(
    database: &Database,
    player: &PlayerState,
) -> Result<PlayerState, DatabaseError> {
    let position = player.position;
    let durable = durable_from_player(player, 1)?;
    let stored = durable.clone();
    database
        .write(move |connection| player_repository::create_player(connection, &stored))
        .await?;
    player_from_durable(durable, position)
}

pub async fn save_player(
    database: &Database,
    original: &PlayerState,
    staged: &PlayerState,
) -> Result<PlayerState, DatabaseError> {
    if staged.revision != original.revision {
        return Err(DatabaseError::Invalid {
            message: "original and staged player revisions differ".to_owned(),
        });
    }
    let revision = next_revision(original.revision)?;
    let original = durable_from_player(original, original.revision)?;
    let position = staged.position;
    let staged = durable_from_player(staged, revision)?;
    require_same_player(&original.id, &staged.id)?;
    let returned = staged.clone();
    database
        .write(move |connection| player_repository::save_player(connection, &original, &staged))
        .await?;
    player_from_durable(returned, position)
}

pub async fn restore_player(
    database: &Database,
    committed: &PlayerState,
    original: &PlayerState,
) -> Result<PlayerState, DatabaseError> {
    let revision = next_revision(committed.revision)?;
    let committed = durable_from_player(committed, committed.revision)?;
    let position = original.position;
    let restored = durable_from_player(original, revision)?;
    require_same_player(&committed.id, &restored.id)?;
    let returned = restored.clone();
    database
        .write(move |connection| player_repository::save_player(connection, &committed, &restored))
        .await?;
    player_from_durable(returned, position)
}

fn next_revision(revision: u64) -> Result<u64, DatabaseError> {
    revision
        .checked_add(1)
        .filter(|revision| i64::try_from(*revision).is_ok())
        .ok_or(DatabaseError::Overflow { field: "revision" })
}

fn require_same_player(
    left: &str,
    right: &str,
) -> Result<(), DatabaseError> {
    if left == right {
        Ok(())
    } else {
        Err(DatabaseError::Invalid {
            message: "original and staged player IDs differ".to_owned(),
        })
    }
}

#[derive(Clone, Copy)]
enum WorkerKind {
    Writer,
    Reader(usize),
}

fn spawn_worker(
    path: PathBuf,
    kind: WorkerKind,
) -> Result<(Worker, JoinHandle<()>), DatabaseError> {
    let (command_sender, command_receiver) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
    let (startup_sender, startup_receiver) = std::sync::mpsc::sync_channel(1);
    let name = match kind {
        WorkerKind::Writer => "oozems-sqlite-writer".to_owned(),
        WorkerKind::Reader(index) => format!("oozems-sqlite-reader-{index}"),
    };
    let handle = std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            let connection = open_worker_connection(&path, kind);
            match connection {
                Ok(connection) => {
                    if startup_sender.send(Ok(())).is_ok() {
                        run_worker(connection, command_receiver);
                    }
                }
                Err(error) => {
                    let _ = startup_sender.send(Err(error));
                }
            }
        })
        .map_err(|error| DatabaseError::Worker {
            message: format!("could not spawn SQLite worker: {error}"),
        })?;
    match startup_receiver.recv() {
        Ok(Ok(())) => Ok((
            Worker {
                sender: command_sender,
            },
            handle,
        )),
        Ok(Err(error)) => {
            let _ = handle.join();
            Err(error)
        }
        Err(_) => {
            let panic = handle.join().err().map(panic_message);
            Err(DatabaseError::Worker {
                message: panic.unwrap_or_else(|| "worker stopped during startup".to_owned()),
            })
        }
    }
}

fn open_worker_connection(
    path: &Path,
    kind: WorkerKind,
) -> Result<Connection, DatabaseError> {
    match kind {
        WorkerKind::Writer => {
            verify_sqlite_version()?;
            let connection = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            configure_writer(&connection)?;
            install_schema(&connection)?;
            verify_schema(&connection)?;
            Ok(connection)
        }
        WorkerKind::Reader(_) => {
            let connection = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            configure_reader(&connection)?;
            verify_schema_identity(&connection)?;
            Ok(connection)
        }
    }
}

fn verify_sqlite_version() -> Result<(), DatabaseError> {
    let version = rusqlite::version_number();
    if version < MINIMUM_SQLITE_VERSION {
        return Err(DatabaseError::Schema {
            message: format!(
                "bundled SQLite 3.51.3 or newer is required, found {}",
                rusqlite::version()
            ),
        });
    }
    Ok(())
}

fn configure_writer(connection: &Connection) -> Result<(), DatabaseError> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "trusted_schema", false)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    let journal_mode: String =
        connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(DatabaseError::Schema {
            message: format!("could not enable WAL mode; SQLite selected {journal_mode:?}"),
        });
    }
    Ok(())
}

fn configure_reader(connection: &Connection) -> Result<(), DatabaseError> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "trusted_schema", false)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(())
}

fn install_schema(connection: &Connection) -> Result<(), DatabaseError> {
    let application_id: i32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let user_version: i32 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match (application_id, user_version) {
        (0, 0) => {
            if let Err(error) = connection.execute_batch(MIGRATION_V1) {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(DatabaseError::Schema {
                    message: format!("could not install schema v1: {error}"),
                });
            }
            apply_migration_v2(connection)?;
        }
        (APPLICATION_ID, 1) => apply_migration_v2(connection)?,
        (APPLICATION_ID, SCHEMA_VERSION) => {}
        (APPLICATION_ID, version) => {
            return Err(DatabaseError::Schema {
                message: format!("unsupported schema version {version}; expected {SCHEMA_VERSION}"),
            });
        }
        (found, version) => {
            return Err(DatabaseError::Schema {
                message: format!(
                    "file is not an oozems database (application_id={found}, \
                     user_version={version})"
                ),
            });
        }
    }
    verify_schema_identity(connection)
}

fn apply_migration_v2(connection: &Connection) -> Result<(), DatabaseError> {
    if let Err(error) = connection.execute_batch(MIGRATION_V2) {
        let _ = connection.execute_batch("ROLLBACK");
        return Err(DatabaseError::Schema {
            message: format!("could not install schema v2: {error}"),
        });
    }
    Ok(())
}

fn verify_schema_identity(connection: &Connection) -> Result<(), DatabaseError> {
    let application_id: i32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let user_version: i32 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if application_id != APPLICATION_ID || user_version != SCHEMA_VERSION {
        return Err(DatabaseError::Schema {
            message: format!(
                "schema identity mismatch (application_id={application_id}, \
                 user_version={user_version})"
            ),
        });
    }
    Ok(())
}

fn verify_schema(connection: &Connection) -> Result<(), DatabaseError> {
    verify_schema_identity(connection)?;
    let required_tables = [
        "players",
        "inventory_stacks",
        "equipped_items",
        "learned_skills",
        "key_bindings",
        "player_quests",
        "quest_mob_progress",
        "quest_records",
        "quest_record_entries",
        "monster_book_cards",
    ];
    for table in required_tables {
        let strict = connection
            .query_row(
                "SELECT strict FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
                [table],
                |row| row.get::<_, i32>(0),
            )
            .map_err(|error| DatabaseError::Schema {
                message: format!("required table {table:?} is missing: {error}"),
            })?;
        if strict != 1 {
            return Err(DatabaseError::Schema {
                message: format!("required table {table:?} is not STRICT"),
            });
        }
    }
    let integrity: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(DatabaseError::Schema {
            message: format!("SQLite quick_check failed: {integrity}"),
        });
    }
    let foreign_key_violation = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()?;
    if foreign_key_violation.is_some() {
        return Err(DatabaseError::Schema {
            message: "SQLite foreign_key_check found a violation".to_owned(),
        });
    }
    Ok(())
}

fn run_worker(
    mut connection: Connection,
    mut commands: mpsc::Receiver<WorkerCommand>,
) {
    while let Some(command) = commands.blocking_recv() {
        match command {
            WorkerCommand::Execute(job) => {
                let _ = std::panic::catch_unwind(AssertUnwindSafe(|| job(&mut connection)));
            }
            WorkerCommand::Shutdown(acknowledgement) => {
                if let Some(acknowledgement) = acknowledgement {
                    let _ = acknowledgement.send(());
                }
                break;
            }
        }
    }
}

fn stop_workers(
    writer: &Worker,
    readers: &[Worker],
    handles: Vec<JoinHandle<()>>,
) {
    let _ = writer.sender.try_send(WorkerCommand::Shutdown(None));
    for reader in readers {
        let _ = reader.sender.try_send(WorkerCommand::Shutdown(None));
    }
    for handle in handles {
        let _ = handle.join();
    }
}

fn panic_message(panic: Box<dyn Any + Send + 'static>) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_owned()
    }
}
