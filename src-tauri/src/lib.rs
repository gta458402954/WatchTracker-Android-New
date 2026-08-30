mod app_paths;
mod auth;
mod collections;
mod commands;
mod db;
mod db_atomic_crud;
mod db_atomic_helpers;
mod db_atomic_update;
mod episode_history;
mod error;
mod local_export;
mod local_import;
mod metadata_identity;
mod models;
mod net;
mod poster_cache;
mod record_validation;
mod recovery_points;
mod secret_store;
mod sync_staging;
mod sync_state;
mod sync_targets;

#[cfg(test)]
mod db_atomic_tests;

use app_paths::AppPaths;
use tauri::Manager;

fn setup_logging(paths: &AppPaths) -> Result<(), String> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}][{}] {}",
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.target(),
                record.level(),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .chain(std::io::stdout())
        .chain(fern::log_file(paths.log()).map_err(|error| {
            format!(
                "Could not open the application log at {}: {error}",
                paths.log().display()
            )
        })?)
        .apply()
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .register_uri_scheme_protocol("poster", |context, request| {
            let path = request.uri().path();
            let file_name = path.trim_start_matches('/');

            // `poster://` 只应访问海报目录中的单个文件。拒绝目录、绝对路径和
            // `..`，避免来自导入数据的 posterPath 跳出 posters 目录。
            let paths = context.app_handle().state::<AppPaths>();
            let full_path = match paths.poster_file(file_name) {
                Ok(path) => path,
                Err(_) => {
                    return tauri::http::Response::builder()
                        .status(400)
                        .body(Vec::new())
                        .unwrap();
                }
            };

            if !full_path.is_file() {
                return tauri::http::Response::builder()
                    .status(404)
                    .body(Vec::new())
                    .unwrap();
            }

            let (content, mime_type) = match poster_cache::read_valid(&full_path) {
                Ok(value) => value,
                Err(_) => {
                    return tauri::http::Response::builder()
                        .status(404)
                        .body(Vec::new())
                        .unwrap();
                }
            };

            tauri::http::Response::builder()
                .header("Content-Type", mime_type)
                .body(content)
                .unwrap()
        })
        .setup(|app| {
            let paths = AppPaths::resolve(app.handle())?;
            setup_logging(&paths)?;
            if let Err(error) = recovery_points::cleanup_temporary_files(&paths) {
                log::warn!("Could not clean stale recovery-point temporary files: {error}");
            }
            if let Err(error) = local_export::cleanup_stale(&paths) {
                log::warn!("Could not clean stale local-export staging files: {error}");
            }
            if let Err(error) = local_import::cleanup_stale(&paths) {
                log::warn!("Could not clean stale local-import staging files: {error}");
            }
            log::info!(
                "Application starting with {} data root: {} (database: {}, posters: {}, backups: {})",
                paths.mode().as_str(),
                paths.root().display(),
                paths.database().display(),
                paths.posters().display(),
                paths.backups().display()
            );

            let db_state = db::init(&paths)?;
            if let Ok(conn) = db_state.conn.lock() {
                if let Ok(records) = db::get_all_records(&conn) {
                    if let Err(error) = poster_cache::cleanup_stale_temporary_files(&paths) {
                        log::warn!("Could not clean stale poster temporary files: {error}");
                    }
                    if let Err(error) = poster_cache::enforce_capacity(&paths, &records) {
                        log::warn!("Could not enforce poster cache capacity: {error}");
                    }
                }
            }

            app.manage(paths);
            app.manage(db_state);
            app.manage(poster_cache::PosterDownloadState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_all_records,
            commands::m0_prepare_poster,
            commands::get_database_compatibility,
            commands::insert_record,
            commands::update_record,
            commands::complete_missing_tmdb_identity,
            commands::delete_record,
            commands::get_collections,
            commands::get_collection_members,
            commands::create_collection,
            commands::create_collection_for_record,
            commands::update_collection,
            commands::delete_collection,
            commands::add_collection_members,
            commands::apply_collection_suggestion,
            commands::create_missing_seasons,
            commands::complete_movie_collection,
            commands::remove_collection_member,
            commands::reorder_collection_members,
            commands::get_episode_tracking,
            commands::get_all_episode_completions,
            commands::get_local_export_snapshot,
            commands::stage_local_export,
            commands::discard_local_export_stage,
            commands::preview_local_import,
            commands::commit_local_import,
            commands::discard_local_import_stage,
            commands::enable_episode_tracking,
            commands::set_next_episode,
            commands::replace_library,
            commands::replace_library_v3,
            commands::replace_all_records,
            commands::get_sync_snapshot,
            commands::get_sync_runtime_state,
            commands::get_sync_targets,
            commands::get_active_sync_connection,
            commands::activate_sync_target,
            commands::disconnect_sync_target,
            commands::set_auto_sync_paused,
            commands::record_sync_failure,
            commands::record_sync_remote_unchanged,
            commands::commit_sync_result,
            commands::prepare_sync_publish_intent,
            commands::resolve_sync_conflict,
            commands::create_recovery_point,
            commands::list_recovery_points,
            commands::set_recovery_point_retained,
            commands::delete_recovery_point,
            commands::restore_recovery_point,
            commands::open_backup_directory,
            commands::get_setting,
            commands::set_setting,
            commands::vacuum_db,
            commands::get_tmdb_credential_status,
            commands::save_tmdb_credential,
            commands::clear_tmdb_credential,
            commands::search_tmdb,
            commands::get_tmdb_detail,
            commands::get_tmdb_season_detail,
            commands::download_poster,
            commands::get_poster_cache_stats,
            commands::clean_poster_cache,
            commands::webdav_request,
            commands::probe_webdav_request,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
