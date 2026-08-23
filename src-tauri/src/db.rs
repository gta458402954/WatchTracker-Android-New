use crate::app_paths::AppPaths;
use crate::models::WatchRecord;
use crate::recovery_points;
#[cfg(test)]
use rusqlite::OpenFlags;
use rusqlite::{params, Connection, OptionalExtension, Result, Row};
use serde::Serialize;
use std::collections::HashSet;
#[cfg(test)]
use std::path::PathBuf;

pub const CURRENT_DB_VERSION: i32 = 18;
const KNOWN_DOWNGRADE_VERSION: i32 = 19;

const V19_COLUMN_RENAMES: [(&str, &str); 21] = [
    ("originalName", "original_name"),
    ("chineseName", "chinese_name"),
    ("totalEpisodes", "total_episodes"),
    ("movieProgress", "movie_progress"),
    ("movieDuration", "movie_duration"),
    ("releaseYear", "release_year"),
    ("posterPath", "poster_path"),
    ("startDate", "start_date"),
    ("endDate", "end_date"),
    ("createdAt", "created_at"),
    ("updatedAt", "updated_at"),
    ("imdbId", "imdb_id"),
    ("isLocked", "is_locked"),
    ("originCountry", "origin_country"),
    ("imdbRating", "imdb_rating"),
    ("tmdbStatus", "tmdb_status"),
    ("interestLevel", "interest_level"),
    ("episodeRuntime", "episode_runtime"),
    ("mediaType", "media_type"),
    ("contentTags", "content_tags"),
    ("revActor", "rev_actor"),
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseCompatibilityIssue {
    pub code: String,
    pub detected_version: i32,
    pub supported_version: i32,
}

pub struct DbState {
    pub conn: std::sync::Mutex<Connection>,
    pub compatibility_issue: Option<DatabaseCompatibilityIssue>,
}

pub fn init(paths: &AppPaths) -> Result<DbState, String> {
    let conn = Connection::open(paths.database()).map_err(|e| {
        format!(
            "Could not open the database at {}: {e}",
            paths.database().display()
        )
    })?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|error| error.to_string())?;

    let current_version = existing_db_version(&conn).map_err(|error| error.to_string())?;
    if current_version > KNOWN_DOWNGRADE_VERSION {
        log::error!(
            "Refusing unsupported database version V{}; this build supports V{} and can only downgrade known V19 databases",
            current_version,
            CURRENT_DB_VERSION,
        );
        return Ok(blocked_state(
            conn,
            "unsupported_newer_database",
            current_version,
        ));
    }

    if current_version == KNOWN_DOWNGRADE_VERSION {
        let backup = match recovery_points::create(&conn, paths, "migration") {
            Ok(backup) => backup,
            Err(error) => {
                log::error!("V19 backup failed before downgrade: {error}");
                return Ok(blocked_state(conn, "v19_downgrade_failed", current_version));
            }
        };
        if let Err(error) = downgrade_v19_to_v18(&conn) {
            log::error!(
                "V19 to V18 downgrade failed and was rolled back; verified backup remains at {}: {error}",
                backup.id,
            );
            return Ok(blocked_state(conn, "v19_downgrade_failed", current_version));
        }
        log::info!(
            "Database safely downgraded from V19 to V18; verified pre-migration backup: {}",
            backup.id,
        );
    }

    let needs_episode_history_backup = existing_db_version(&conn).unwrap_or(0) > 0
        && !episode_history_schema_ready(&conn).unwrap_or(false);
    if needs_episode_history_backup {
        recovery_points::create(&conn, paths, "episode-history-migration")
            .map_err(|error| format!("episode_history_migration_failed:{error}"))?;
    }
    let needs_collections_backup = existing_db_version(&conn).unwrap_or(0) > 0
        && !crate::collections::schema_ready(&conn).unwrap_or(false);
    if needs_collections_backup {
        recovery_points::create(&conn, paths, "series-identity-migration")
            .map_err(|error| format!("collections_migration_failed:{error}"))?;
    }

    // 初始化表逻辑
    setup_db(&conn).map_err(|e| e.to_string())?;

    Ok(DbState {
        conn: std::sync::Mutex::new(conn),
        compatibility_issue: None,
    })
}

fn blocked_state(conn: Connection, code: &str, detected_version: i32) -> DbState {
    DbState {
        conn: std::sync::Mutex::new(conn),
        compatibility_issue: Some(DatabaseCompatibilityIssue {
            code: code.to_string(),
            detected_version,
            supported_version: CURRENT_DB_VERSION,
        }),
    }
}

fn existing_db_version(conn: &Connection) -> Result<i32> {
    let has_settings: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='settings')",
        [],
        |row| row.get(0),
    )?;
    if !has_settings {
        return Ok(0);
    }
    let value = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'db_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(value.and_then(|raw| raw.parse().ok()).unwrap_or(0))
}

fn record_columns(conn: &Connection) -> Result<HashSet<String>> {
    let mut statement = conn.prepare("SELECT name FROM pragma_table_info('records')")?;
    let columns = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<HashSet<_>>>()?;
    Ok(columns)
}

fn downgrade_v19_to_v18(conn: &Connection) -> Result<()> {
    let columns = record_columns(conn)?;
    if V19_COLUMN_RENAMES
        .iter()
        .any(|(camel, snake)| columns.contains(*camel) || !columns.contains(*snake))
    {
        return Err(rusqlite::Error::InvalidQuery);
    }

    let transaction = conn.unchecked_transaction()?;
    for (camel, snake) in V19_COLUMN_RENAMES {
        transaction.execute(
            &format!("ALTER TABLE records RENAME COLUMN {snake} TO {camel}"),
            [],
        )?;
    }
    let converted_columns = record_columns(&transaction)?;
    if V19_COLUMN_RENAMES.iter().any(|(camel, snake)| {
        !converted_columns.contains(*camel) || converted_columns.contains(*snake)
    }) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    transaction.execute(
        "INSERT INTO settings (key, value) VALUES ('db_version', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [CURRENT_DB_VERSION.to_string()],
    )?;
    transaction.execute(
        "INSERT INTO settings (key, value) VALUES ('database_migration_notice', 'v19_to_v18') \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [],
    )?;
    let integrity: String =
        transaction.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(rusqlite::Error::InvalidQuery);
    }
    transaction.commit()?;
    Ok(())
}

struct Migration {
    version: i32,
    up: fn(&Connection) -> Result<()>,
}

pub(crate) fn setup_db(conn: &Connection) -> Result<()> {
    let detected_version = existing_db_version(conn)?;
    if detected_version > CURRENT_DB_VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }
    conn.execute(
        "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT);",
        [],
    )?;

    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = 'db_version'")?;
    let current_version: i32 = stmt
        .query_row([], |row| {
            let val: String = row.get(0)?;
            Ok(val.parse().unwrap_or(0))
        })
        .unwrap_or(0);

    let migrations = [
        Migration {
            version: 1,
            up: |conn| {
                conn.execute_batch("
                    CREATE TABLE IF NOT EXISTS records (
                      id TEXT PRIMARY KEY, originalName TEXT, chineseName TEXT, progress TEXT, totalEpisodes INTEGER, status TEXT, platform TEXT, rating INTEGER, startDate TEXT, endDate TEXT, category TEXT, notes TEXT, createdAt TEXT
                    );
                ")
            },
        },
        Migration {
            version: 2,
            up: |conn| {
                let cols = [
                    ("movieProgress", "INTEGER"),
                    ("movieDuration", "INTEGER"),
                    ("releaseYear", "TEXT"),
                    ("posterPath", "TEXT"),
                    ("updatedAt", "TEXT"),
                ];
                for (name, col_type) in cols {
                    let exists: bool = conn
                        .query_row(
                            &format!(
                                "SELECT count(*) FROM pragma_table_info('records') WHERE name='{}'",
                                name
                            ),
                            [],
                            |row| row.get(0),
                        )
                        .unwrap_or(false);
                    if !exists {
                        conn.execute(
                            &format!("ALTER TABLE records ADD COLUMN {} {}", name, col_type),
                            [],
                        )?;
                    }
                }
                Ok(())
            },
        },
        Migration {
            version: 4,
            up: |conn| {
                let exists: bool = conn
                    .query_row(
                        "SELECT count(*) FROM pragma_table_info('records') WHERE name='imdbId'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                if !exists {
                    conn.execute("ALTER TABLE records ADD COLUMN imdbId TEXT", [])?;
                }
                Ok(())
            },
        },
        Migration {
            version: 5,
            up: |conn| {
                let exists: bool = conn
                    .query_row(
                        "SELECT count(*) FROM pragma_table_info('records') WHERE name='isLocked'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                if !exists {
                    conn.execute(
                        "ALTER TABLE records ADD COLUMN isLocked INTEGER DEFAULT 0",
                        [],
                    )?;
                }
                Ok(())
            },
        },
        Migration {
            version: 7,
            up: |conn| {
                let cols = [
                    ("genres", "TEXT"),
                    ("originCountry", "TEXT"),
                    ("imdbRating", "REAL"),
                    ("tmdbStatus", "TEXT"),
                    ("interestLevel", "INTEGER"),
                ];
                for (name, col_type) in cols {
                    let exists: bool = conn
                        .query_row(
                            &format!(
                                "SELECT count(*) FROM pragma_table_info('records') WHERE name='{}'",
                                name
                            ),
                            [],
                            |row| row.get(0),
                        )
                        .unwrap_or(false);
                    if !exists {
                        conn.execute(
                            &format!("ALTER TABLE records ADD COLUMN {} {}", name, col_type),
                            [],
                        )?;
                    }
                }
                Ok(())
            },
        },
        Migration {
            version: 8,
            up: |conn| {
                let exists: bool = conn
                    .query_row(
                        "SELECT count(*) FROM pragma_table_info('records') WHERE name='episodeRuntime'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                if !exists {
                    conn.execute("ALTER TABLE records ADD COLUMN episodeRuntime INTEGER", [])?;
                }
                Ok(())
            },
        },
        Migration {
            version: 9,
            up: |conn| {
                conn.execute("UPDATE records SET rating = rating * 2 WHERE rating IS NOT NULL AND rating >= 1 AND rating <= 5;", [])?;
                Ok(())
            },
        },
        Migration {
            version: 10,
            up: |conn| {
                conn.execute("ALTER TABLE records ADD COLUMN mediaType TEXT", [])
                    .ok();
                conn.execute("ALTER TABLE records ADD COLUMN contentTags TEXT", [])
                    .ok();
                conn.execute("UPDATE records SET mediaType = CASE WHEN category IN ('电影', '纪录片') THEN '电影' WHEN category = '综艺' THEN '综艺' WHEN category = '动画' THEN '动画' ELSE '剧集' END WHERE mediaType IS NULL OR mediaType = ''", [])?;
                conn.execute("UPDATE records SET contentTags = CASE WHEN contentTags IS NULL OR contentTags = '' THEN CASE WHEN category = '纪录片' THEN '纪录片' ELSE '' END ELSE contentTags END", [])?;
                Ok(())
            },
        },
        Migration {
            version: 11,
            up: |conn| {
                conn.execute("UPDATE records SET contentTags = CASE category WHEN '美剧' THEN '美国' WHEN '英剧' THEN '英国' WHEN '日剧' THEN '日本' WHEN '韩剧' THEN '韩国' WHEN '国产剧' THEN '中国大陆' WHEN '港剧' THEN '中国香港' WHEN '台剧' THEN '中国台湾' WHEN '纪录片' THEN '纪录片' ELSE contentTags END WHERE contentTags IS NULL OR contentTags = ''", [])?;
                Ok(())
            },
        },
        Migration {
            version: 12,
            up: |conn| {
                conn.execute(
                    "UPDATE records SET mediaType = '纪录片' WHERE category = '纪录片'",
                    [],
                )?;
                Ok(())
            },
        },
        Migration {
            version: 13,
            up: |conn| {
                conn.execute_batch("
                    UPDATE records SET contentTags = TRIM(REPLACE(',' || contentTags || ',', ',纪录片,', ','), ', ') WHERE contentTags LIKE '%纪录片%';
                    DROP TABLE IF EXISTS categories;
                    DROP TABLE IF EXISTS watch_logs;
                    DROP INDEX IF EXISTS idx_records_chineseName;
                    DROP INDEX IF EXISTS idx_records_originalName;
                ")
            },
        },
        Migration {
            version: 14,
            up: |conn| {
                conn.execute_batch("
                    CREATE TABLE records_v14 (
                        id TEXT PRIMARY KEY,
                        originalName TEXT NOT NULL DEFAULT '',
                        chineseName TEXT NOT NULL DEFAULT '',
                        progress TEXT NOT NULL DEFAULT '',
                        totalEpisodes INTEGER,
                        status TEXT NOT NULL DEFAULT '未看',
                        platform TEXT NOT NULL DEFAULT '',
                        rating INTEGER,
                        startDate TEXT,
                        endDate TEXT,
                        notes TEXT NOT NULL DEFAULT '',
                        createdAt TEXT NOT NULL DEFAULT '',
                        movieProgress INTEGER,
                        movieDuration INTEGER,
                        releaseYear TEXT,
                        posterPath TEXT,
                        updatedAt TEXT,
                        imdbId TEXT,
                        isLocked INTEGER DEFAULT 0,
                        genres TEXT,
                        originCountry TEXT,
                        imdbRating REAL,
                        tmdbStatus TEXT,
                        interestLevel INTEGER,
                        episodeRuntime INTEGER,
                        mediaType TEXT NOT NULL DEFAULT '电影',
                        contentTags TEXT
                    );
                    INSERT INTO records_v14 (
                        id, originalName, chineseName, progress, totalEpisodes, status, platform, rating,
                        startDate, endDate, notes, createdAt, movieProgress, movieDuration, releaseYear,
                        posterPath, updatedAt, imdbId, isLocked, genres, originCountry, imdbRating,
                        tmdbStatus, interestLevel, episodeRuntime, mediaType, contentTags
                    )
                    SELECT
                        id, COALESCE(originalName, ''), COALESCE(chineseName, ''), COALESCE(progress, ''),
                        totalEpisodes, COALESCE(status, '未看'), COALESCE(platform, ''), rating,
                        startDate, endDate, COALESCE(notes, ''), COALESCE(createdAt, ''), movieProgress,
                        movieDuration, releaseYear, posterPath, updatedAt, imdbId, COALESCE(isLocked, 0),
                        genres, originCountry, imdbRating, tmdbStatus, interestLevel, episodeRuntime,
                        COALESCE(NULLIF(mediaType, ''), '电影'), contentTags
                    FROM records;
                    DROP TABLE records;
                    ALTER TABLE records_v14 RENAME TO records;
                ")
            },
        },
        Migration {
            version: 15,
            up: |conn| {
                conn.execute_batch(
                    "UPDATE records SET status = CASE
                        WHEN lower(trim(status)) IN ('watched', 'completed', 'finish', 'finished', 'done', '已看') THEN '已看'
                        WHEN lower(trim(status)) IN ('watching', 'doing', 'start', 'started', '在看') THEN '在看'
                        ELSE '未看'
                    END;"
                )
            },
        },
        Migration {
            version: 16,
            up: |conn| {
                conn.execute_batch(
                    "CREATE INDEX IF NOT EXISTS idx_records_createdAt ON records(createdAt);
                     CREATE INDEX IF NOT EXISTS idx_records_status ON records(status);
                     CREATE INDEX IF NOT EXISTS idx_records_mediaType ON records(mediaType);",
                )
            },
        },
        Migration {
            version: 17,
            up: |conn| {
                let exists: bool = conn.query_row(
                    "SELECT count(*) FROM pragma_table_info('records') WHERE name = 'rev'",
                    [],
                    |row| row.get(0),
                )?;
                if !exists {
                    conn.execute(
                        "ALTER TABLE records ADD COLUMN rev INTEGER NOT NULL DEFAULT 0",
                        [],
                    )?;
                }
                Ok(())
            },
        },
        Migration {
            version: 18,
            up: |conn| {
                let exists: bool = conn.query_row(
                    "SELECT count(*) FROM pragma_table_info('records') WHERE name = 'revActor'",
                    [],
                    |row| row.get(0),
                )?;
                if !exists {
                    conn.execute(
                        "ALTER TABLE records ADD COLUMN revActor TEXT NOT NULL DEFAULT ''",
                        [],
                    )?;
                }
                conn.execute(
                    "INSERT INTO settings (key, value) VALUES ('records_generation', '0') \
                     ON CONFLICT(key) DO NOTHING",
                    [],
                )?;
                Ok(())
            },
        },
    ];

    for m in migrations {
        if current_version < m.version {
            let transaction = conn.unchecked_transaction()?;
            (m.up)(&transaction)?;
            transaction.execute(
                "INSERT INTO settings (key, value) VALUES ('db_version', ?1) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![m.version.to_string()],
            )?;
            transaction.commit()?;
        }
    }

    migrate_episode_history_schema(conn)?;
    crate::collections::migrate_schema(conn)?;

    Ok(())
}

fn episode_history_schema_ready(conn: &Connection) -> Result<bool> {
    let columns = record_columns(conn)?;
    let table_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='episode_completions')",
        [],
        |row| row.get(0),
    )?;
    let index_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='episode_completions_record_episode')",
        [],
        |row| row.get(0),
    )?;
    let marker = conn
        .query_row(
            "SELECT value FROM settings WHERE key='episode_history_schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(columns.contains("episodeTrackingEnabled")
        && columns.contains("nextEpisode")
        && table_exists
        && index_exists
        && marker.as_deref() == Some("1"))
}

pub(crate) fn migrate_episode_history_schema(conn: &Connection) -> Result<()> {
    let transaction = conn.unchecked_transaction()?;
    let columns = record_columns(&transaction)?;
    if !columns.contains("episodeTrackingEnabled") {
        transaction.execute(
            "ALTER TABLE records ADD COLUMN episodeTrackingEnabled INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !columns.contains("nextEpisode") {
        transaction.execute("ALTER TABLE records ADD COLUMN nextEpisode INTEGER", [])?;
    }
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS episode_completions (
            id TEXT PRIMARY KEY,
            recordId TEXT NOT NULL,
            episodeNumber INTEGER NOT NULL CHECK (episodeNumber > 0),
            completedAt TEXT,
            createdAt TEXT NOT NULL,
            updatedAt TEXT NOT NULL,
            rev INTEGER NOT NULL DEFAULT 0,
            revActor TEXT NOT NULL DEFAULT '',
            UNIQUE(recordId, episodeNumber),
            FOREIGN KEY(recordId) REFERENCES records(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS episode_completions_record_episode
           ON episode_completions(recordId, episodeNumber);
         INSERT INTO settings(key, value) VALUES('episode_history_schema_version', '1')
           ON CONFLICT(key) DO UPDATE SET value=excluded.value;
         INSERT INTO settings(key, value) VALUES('required_app_features_v1', '[\"episode-history-v1\"]')
           ON CONFLICT(key) DO UPDATE SET value=excluded.value;",
    )?;
    transaction.commit()?;
    Ok(())
}

// 命令实现
fn map_row_to_record(row: &Row<'_>) -> Result<WatchRecord> {
    Ok(WatchRecord {
        id: row.get("id")?,
        original_name: row.get("originalName")?,
        chinese_name: row.get("chineseName")?,
        progress: row.get("progress")?,
        total_episodes: row.get("totalEpisodes")?,
        episode_tracking_enabled: row
            .get::<_, Option<i32>>("episodeTrackingEnabled")
            .unwrap_or(None)
            .unwrap_or(0)
            != 0,
        next_episode: row.get("nextEpisode").unwrap_or(None),
        status: row.get("status")?,
        platform: row.get("platform")?,
        rating: row.get("rating").unwrap_or(None),
        start_date: row.get("startDate")?,
        end_date: row.get("endDate")?,
        notes: row.get("notes")?,
        created_at: row.get("createdAt")?,
        movie_progress: row.get("movieProgress").unwrap_or(None),
        movie_duration: row.get("movieDuration").unwrap_or(None),
        release_year: row
            .get::<_, String>("releaseYear")
            .map(Some)
            .or_else(|_| {
                row.get::<_, i32>("releaseYear")
                    .map(|value| Some(value.to_string()))
            })
            .unwrap_or(None),
        poster_path: row.get("posterPath").unwrap_or(None),
        updated_at: row.get("updatedAt").unwrap_or(None),
        imdb_id: row.get("imdbId").unwrap_or(None),
        is_locked: row
            .get::<_, Option<i32>>("isLocked")
            .unwrap_or(None)
            .map(|value| value != 0),
        genres: row.get("genres").unwrap_or(None),
        origin_country: row.get("originCountry").unwrap_or(None),
        imdb_rating: row.get("imdbRating").unwrap_or(None),
        tmdb_status: row.get("tmdbStatus").unwrap_or(None),
        interest_level: row.get("interestLevel").unwrap_or(None),
        episode_runtime: row.get("episodeRuntime").unwrap_or(None),
        media_type: row.get("mediaType").unwrap_or_else(|_| "电影".to_string()),
        content_tags: row.get("contentTags").unwrap_or(None),
        tmdb_media_kind: row.get("tmdbMediaKind").unwrap_or(None),
        tmdb_id: row.get("tmdbId").unwrap_or(None),
        tmdb_parent_id: row.get("tmdbParentId").unwrap_or(None),
        tmdb_season_number: row.get("tmdbSeasonNumber").unwrap_or(None),
        series_record_kind: row.get("seriesRecordKind").unwrap_or(None),
        rev: row.get("rev").unwrap_or(0),
        rev_actor: row.get("revActor").unwrap_or_default(),
    })
}

pub fn get_all_records(conn: &Connection) -> Result<Vec<WatchRecord>> {
    let mut stmt = conn.prepare("SELECT * FROM records ORDER BY createdAt DESC")?;

    // 使用列名映射，避免索引错误
    let rows = stmt.query_map([], map_row_to_record)?;

    let mut results = Vec::new();
    for row in rows {
        match row {
            Ok(r) => results.push(r),
            Err(e) => eprintln!("[DB] Failed to read record row: {}", e),
        }
    }
    Ok(results)
}

pub fn get_record(conn: &Connection, id: &str) -> Result<Option<WatchRecord>> {
    conn.query_row(
        "SELECT * FROM records WHERE id = ?1",
        [id],
        map_row_to_record,
    )
    .optional()
}

pub fn insert_record(conn: &Connection, r: WatchRecord) -> Result<()> {
    log::info!(
        "[DB] Inserting/Updating record: {} ({})",
        r.chinese_name,
        r.id
    );
    let is_locked_int = r.is_locked.map(|b| if b { 1 } else { 0 });
    conn.execute(
        "INSERT INTO records (
            id, originalName, chineseName, progress, totalEpisodes, episodeTrackingEnabled, nextEpisode, status, platform, rating,
            startDate, endDate, notes, createdAt, movieProgress, movieDuration, releaseYear,
            posterPath, updatedAt, imdbId, isLocked, genres, originCountry, imdbRating,
            tmdbStatus, interestLevel, episodeRuntime, mediaType, contentTags, tmdbMediaKind, tmdbId,
            tmdbParentId, tmdbSeasonNumber, seriesRecordKind, rev, revActor
         ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
         ON CONFLICT(id) DO UPDATE SET
            originalName = excluded.originalName,
            chineseName = excluded.chineseName,
            progress = excluded.progress,
            totalEpisodes = excluded.totalEpisodes,
            episodeTrackingEnabled = excluded.episodeTrackingEnabled,
            nextEpisode = excluded.nextEpisode,
            status = excluded.status,
            platform = excluded.platform,
            rating = excluded.rating,
            startDate = excluded.startDate,
            endDate = excluded.endDate,
            notes = excluded.notes,
            createdAt = excluded.createdAt,
            movieProgress = excluded.movieProgress,
            movieDuration = excluded.movieDuration,
            releaseYear = excluded.releaseYear,
            posterPath = excluded.posterPath,
            updatedAt = excluded.updatedAt,
            imdbId = excluded.imdbId,
            isLocked = excluded.isLocked,
            genres = excluded.genres,
            originCountry = excluded.originCountry,
            imdbRating = excluded.imdbRating,
            tmdbStatus = excluded.tmdbStatus,
            interestLevel = excluded.interestLevel,
            episodeRuntime = excluded.episodeRuntime,
            mediaType = excluded.mediaType,
            contentTags = excluded.contentTags,
            tmdbMediaKind = excluded.tmdbMediaKind,
            tmdbId = excluded.tmdbId,
            tmdbParentId = excluded.tmdbParentId,
            tmdbSeasonNumber = excluded.tmdbSeasonNumber,
            seriesRecordKind = excluded.seriesRecordKind,
            rev = excluded.rev,
            revActor = excluded.revActor",
        params![
            r.id,
            r.original_name,
            r.chinese_name,
            r.progress,
            r.total_episodes,
            i64::from(r.episode_tracking_enabled),
            r.next_episode,
            r.status,
            r.platform,
            r.rating,
            r.start_date,
            r.end_date,
            r.notes,
            r.created_at,
            r.movie_progress,
            r.movie_duration,
            r.release_year,
            r.poster_path,
            r.updated_at,
            r.imdb_id,
            is_locked_int,
            r.genres,
            r.origin_country,
            r.imdb_rating,
            r.tmdb_status,
            r.interest_level,
            r.episode_runtime,
            r.media_type,
            r.content_tags,
            r.tmdb_media_kind,
            r.tmdb_id,
            r.tmdb_parent_id,
            r.tmdb_season_number,
            r.series_record_kind,
            r.rev,
            r.rev_actor
        ],
    )?;
    Ok(())
}

pub(crate) fn replace_all_records_tx(conn: &Connection, records: Vec<WatchRecord>) -> Result<()> {
    let mut stmt = conn.prepare("SELECT id FROM records WHERE isLocked = 1")?;
    let locked_ids: HashSet<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(Result::ok)
        .collect();

    conn.execute(
        "DELETE FROM records WHERE isLocked IS NULL OR isLocked = 0",
        [],
    )?;

    for record in records {
        if locked_ids.contains(&record.id) {
            log::info!("[DB] Sync skipping locked record: {}", record.id);
            continue;
        }
        insert_record(conn, record)?;
    }
    Ok(())
}

pub fn set_setting(conn: &Connection, key: String, value: String) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn vacuum_db(conn: &Connection) -> Result<()> {
    conn.execute("VACUUM", [])?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: String) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_VERSION_TEST_ID: AtomicU64 = AtomicU64::new(1);

    struct VersionTestRoot(PathBuf);

    impl VersionTestRoot {
        fn new(name: &str) -> (Self, AppPaths) {
            let id = NEXT_VERSION_TEST_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "watchtracker-version-{}-{name}-{id}",
                std::process::id(),
            ));
            let executable_dir = root.join("bin");
            std::fs::create_dir_all(executable_dir.join("data"))
                .expect("create portable test data directory");
            let paths = AppPaths::resolve_from(
                Some(&executable_dir.join("app.exe")),
                &root.join("app-data"),
            )
            .expect("resolve test paths");
            (Self(root), paths)
        }
    }

    impl Drop for VersionTestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn seed_v19_database(paths: &AppPaths, fail_downgrade: bool) {
        let conn = Connection::open(paths.database()).expect("open V19 fixture");
        setup_db(&conn).expect("create V18 fixture");
        insert_record(&conn, record("v19-record", "V19 保留记录", false))
            .expect("insert V19 fixture record");
        for (camel, snake) in V19_COLUMN_RENAMES {
            conn.execute(
                &format!("ALTER TABLE records RENAME COLUMN {camel} TO {snake}"),
                [],
            )
            .expect("rename fixture column to V19");
        }
        conn.execute(
            "UPDATE settings SET value = '19' WHERE key = 'db_version'",
            [],
        )
        .expect("mark fixture as V19");
        if fail_downgrade {
            conn.execute_batch(
                "CREATE TRIGGER fail_v19_downgrade BEFORE UPDATE OF value ON settings
                 WHEN OLD.key = 'db_version' AND NEW.value = '18'
                 BEGIN SELECT RAISE(ABORT, 'injected V19 downgrade failure'); END;",
            )
            .expect("install downgrade failure trigger");
        }
    }

    fn record(id: &str, name: &str, locked: bool) -> WatchRecord {
        WatchRecord {
            id: id.to_string(),
            original_name: name.to_string(),
            chinese_name: name.to_string(),
            progress: String::new(),
            total_episodes: None,
            episode_tracking_enabled: false,
            next_episode: None,
            movie_progress: None,
            movie_duration: Some(7_200),
            release_year: Some("2026".to_string()),
            poster_path: None,
            status: crate::models::RecordStatus::Unwatched,
            platform: String::new(),
            rating: None,
            start_date: None,
            end_date: None,
            notes: String::new(),
            created_at: "2026-07-25T00:00:00.000Z".to_string(),
            updated_at: Some("2026-07-25T00:00:00.000Z".to_string()),
            imdb_id: None,
            is_locked: Some(locked),
            genres: Some("剧情".to_string()),
            origin_country: Some("US".to_string()),
            imdb_rating: Some(8.0),
            tmdb_status: Some("Released".to_string()),
            interest_level: None,
            episode_runtime: Some(120),
            media_type: "电影".to_string(),
            content_tags: Some("美国".to_string()),
            tmdb_media_kind: None,
            tmdb_id: None,
            tmdb_parent_id: None,
            tmdb_season_number: None,
            series_record_kind: None,
            rev: 0,
            rev_actor: String::new(),
        }
    }

    #[test]
    fn schema_v14_and_record_round_trip() {
        let conn = Connection::open_in_memory().expect("open database");
        setup_db(&conn).expect("migrate database");

        let version: String = get_setting(&conn, "db_version".to_string())
            .unwrap()
            .unwrap();
        assert_eq!(version, "18");

        let legacy_tables: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('categories', 'watch_logs')",
                [],
                |row| row.get(0),
            )
            .expect("check legacy tables");
        assert_eq!(legacy_tables, 0);

        let legacy_columns: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('records') WHERE name IN ('category', 'sortOrder')",
                [],
                |row| row.get(0),
            )
            .expect("check legacy columns");
        assert_eq!(legacy_columns, 0);

        insert_record(&conn, record("one", "测试电影", false)).expect("insert record");
        let records = get_all_records(&conn).expect("read records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].media_type, "电影");
        assert_eq!(records[0].content_tags.as_deref(), Some("美国"));
    }

    #[test]
    fn one_incompatible_row_does_not_prevent_loading_valid_records() {
        let conn = Connection::open_in_memory().expect("open database");
        setup_db(&conn).expect("migrate database");
        insert_record(&conn, record("valid", "正常记录", false)).expect("insert valid record");
        insert_record(&conn, record("dirty", "兼容脏记录", false)).expect("insert dirty record");
        conn.execute(
            "UPDATE records SET status = 'legacy-invalid' WHERE id = 'dirty'",
            [],
        )
        .expect("make one row incompatible");

        let records = get_all_records(&conn).expect("load compatible records");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "valid");
    }

    #[test]
    fn settings_round_trip_survives_reopen_on_file_database() {
        let directory =
            std::env::temp_dir().join(format!("watchtracker-settings-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("create temporary directory");
        let path = directory.join("settings.db");
        {
            let conn = Connection::open(&path).expect("open database");
            setup_db(&conn).expect("migrate database");
            set_setting(&conn, "sync_interval".to_string(), "45".to_string())
                .expect("save setting");
        }
        {
            let conn = Connection::open(&path).expect("reopen database");
            setup_db(&conn).expect("verify current database");
            assert_eq!(
                get_setting(&conn, "sync_interval".to_string())
                    .unwrap()
                    .as_deref(),
                Some("45")
            );
        }
        std::fs::remove_file(&path).expect("remove temporary database");
        std::fs::remove_dir(&directory).expect("remove temporary directory");
    }

    #[test]
    fn replace_all_records_preserves_locked_local_record() {
        let mut conn = Connection::open_in_memory().expect("open database");
        setup_db(&conn).expect("migrate database");
        insert_record(&conn, record("locked", "本地锁定版本", true)).expect("insert locked record");
        insert_record(&conn, record("old", "待替换版本", false)).expect("insert old record");

        crate::db_atomic_crud::replace_all_records_atomic(
            &mut conn,
            vec![
                record("locked", "云端覆盖版本", false),
                record("remote", "云端新记录", false),
            ],
        )
        .expect("replace records");

        let records = get_all_records(&conn).expect("read records");
        assert_eq!(records.len(), 2);
        assert_eq!(
            records
                .iter()
                .find(|item| item.id == "locked")
                .map(|item| item.chinese_name.as_str()),
            Some("本地锁定版本"),
        );
        assert!(records.iter().any(|item| item.id == "remote"));
        assert!(!records.iter().any(|item| item.id == "old"));
    }

    #[test]
    fn migrates_v12_database_without_losing_records() {
        let conn = Connection::open_in_memory().expect("open database");
        setup_db(&conn).expect("create current schema");
        insert_record(&conn, record("legacy", "迁移保留测试", false)).expect("insert record");

        conn.execute("ALTER TABLE records ADD COLUMN category TEXT", [])
            .expect("add legacy category");
        conn.execute("ALTER TABLE records ADD COLUMN sortOrder INTEGER", [])
            .expect("add legacy sort order");
        conn.execute(
            "UPDATE records SET category = '韩剧', sortOrder = 7, contentTags = '韩国' WHERE id = 'legacy'",
            [],
        )
        .expect("seed legacy fields");
        conn.execute(
            "UPDATE settings SET value = '12' WHERE key = 'db_version'",
            [],
        )
        .expect("downgrade schema marker");

        setup_db(&conn).expect("migrate legacy schema");
        let records = get_all_records(&conn).expect("read migrated records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "legacy");
        assert_eq!(records[0].media_type, "电影");
        assert_eq!(records[0].content_tags.as_deref(), Some("韩国"));

        let legacy_columns: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('records') WHERE name IN ('category', 'sortOrder')",
                [],
                |row| row.get(0),
            )
            .expect("check removed columns");
        assert_eq!(legacy_columns, 0);
    }

    #[test]
    fn v19_database_is_backed_up_and_atomically_downgraded_to_v18() {
        let (_root, paths) = VersionTestRoot::new("v19-success");
        seed_v19_database(&paths, false);

        let state = init(&paths).expect("initialize known V19 database");
        assert!(state.compatibility_issue.is_none());
        let conn = state.conn.lock().expect("lock downgraded database");
        assert_eq!(existing_db_version(&conn).unwrap(), CURRENT_DB_VERSION);
        let columns = record_columns(&conn).unwrap();
        assert!(columns.contains("chineseName"));
        assert!(!columns.contains("chinese_name"));
        assert_eq!(
            get_all_records(&conn).unwrap()[0].chinese_name,
            "V19 保留记录"
        );
        assert_eq!(
            get_setting(&conn, "database_migration_notice".to_string())
                .unwrap()
                .as_deref(),
            Some("v19_to_v18"),
        );
        drop(conn);

        let backups = std::fs::read_dir(paths.backups())
            .expect("read backup directory")
            .map(|entry| entry.expect("read backup entry").path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("db"))
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        let backup = Connection::open_with_flags(&backups[0], OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open verified backup");
        assert_eq!(
            existing_db_version(&backup).unwrap(),
            KNOWN_DOWNGRADE_VERSION
        );
        let backup_columns = record_columns(&backup).unwrap();
        assert!(backup_columns.contains("chinese_name"));
        assert!(!backup_columns.contains("chineseName"));
    }

    #[test]
    fn v19_downgrade_failure_rolls_back_source_and_keeps_verified_backup() {
        let (_root, paths) = VersionTestRoot::new("v19-rollback");
        seed_v19_database(&paths, true);

        let state = init(&paths).expect("return blocked state after downgrade failure");
        let issue = state
            .compatibility_issue
            .expect("report compatibility issue");
        assert_eq!(issue.code, "v19_downgrade_failed");
        let conn = state.conn.lock().expect("lock rolled back database");
        assert_eq!(existing_db_version(&conn).unwrap(), KNOWN_DOWNGRADE_VERSION);
        let columns = record_columns(&conn).unwrap();
        assert!(columns.contains("chinese_name"));
        assert!(!columns.contains("chineseName"));
        assert_eq!(
            std::fs::read_dir(paths.backups())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(
                    |entry| entry.path().extension().and_then(|value| value.to_str()) == Some("db")
                )
                .count(),
            1,
            "verified pre-migration backup must remain available",
        );
    }

    #[test]
    fn v20_and_newer_database_is_rejected_without_changes_or_backup() {
        let (_root, paths) = VersionTestRoot::new("v20-reject");
        {
            let conn = Connection::open(paths.database()).expect("open V20 fixture");
            setup_db(&conn).expect("create fixture schema");
            insert_record(&conn, record("future", "未来版本记录", false))
                .expect("insert future fixture record");
            conn.execute(
                "UPDATE settings SET value = '20' WHERE key = 'db_version'",
                [],
            )
            .expect("mark fixture as V20");
        }
        let before = std::fs::read(paths.database()).expect("read fixture before rejection");

        let state = init(&paths).expect("return blocked state for V20");
        let issue = state.compatibility_issue.expect("report newer database");
        assert_eq!(issue.code, "unsupported_newer_database");
        assert_eq!(issue.detected_version, 20);
        assert_eq!(issue.supported_version, CURRENT_DB_VERSION);
        assert_eq!(
            std::fs::read(paths.database()).expect("read fixture after rejection"),
            before,
        );
        assert_eq!(std::fs::read_dir(paths.backups()).unwrap().count(), 0);
    }
}
