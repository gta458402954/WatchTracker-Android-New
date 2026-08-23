use crate::app_paths::AppPaths;
use crate::poster_cache::{self, PosterDownloadResult, PosterDownloadState};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::time::Duration;
use uuid::Uuid;

const TMDB_JSON_MAX_BYTES: usize = 4 * 1024 * 1024;
const WEBDAV_JSON_MAX_BYTES: usize = 64 * 1024 * 1024;
const WEBDAV_XML_MAX_BYTES: usize = 1024 * 1024;

async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
    error_code: &str,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(error_code.to_string());
    }
    let mut body =
        Vec::with_capacity(response.content_length().unwrap_or(0).min(limit as u64) as usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "network_body_read_failed".to_string())?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(error_code.to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn json_response(
    response: reqwest::Response,
    limit: usize,
    error_code: &str,
    validate_mime: bool,
) -> Result<Value, String> {
    if validate_mime && response.status().is_success() {
        if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
            let content_type = content_type
                .to_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !content_type.contains("json") && !content_type.contains("octet-stream") {
                return Err("network_content_type_invalid".to_string());
            }
        }
    }
    let bytes = read_limited(response, limit, error_code).await?;
    serde_json::from_slice(&bytes).map_err(|_| "network_json_invalid".to_string())
}

fn client_builder(
    proxy: Option<String>,
    timeout: Duration,
) -> Result<reqwest::ClientBuilder, String> {
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout);
    if let Some(proxy) = proxy.filter(|value| !value.trim().is_empty()) {
        builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|error| error.to_string())?);
    }
    Ok(builder)
}

pub async fn search_tmdb(
    api_key: String,
    query: String,
    language: String,
    proxy: Option<String>,
) -> Result<Value, String> {
    let is_imdb_id = query.starts_with("tt")
        && query.len() > 2
        && query[2..].chars().all(|c| c.is_ascii_digit());
    if is_imdb_id {
        log::info!("[TMDB] IMDb ID Searching for: {}", query);
    } else {
        log::info!("[TMDB] Multi Searching for: {}", query);
    }

    let builder = client_builder(proxy, Duration::from_secs(30))?;
    let client = builder.build().map_err(|e| e.to_string())?;

    let is_jwt = api_key.contains('.') || api_key.len() > 40;

    let url = if is_imdb_id {
        if is_jwt {
            format!(
                "https://api.themoviedb.org/3/find/{}?external_source=imdb_id&language={}",
                query, language
            )
        } else {
            format!("https://api.themoviedb.org/3/find/{}?api_key={}&external_source=imdb_id&language={}", query, api_key, language)
        }
    } else {
        if is_jwt {
            format!(
                "https://api.themoviedb.org/3/search/multi?query={}&language={}&include_adult=false",
                urlencoding::encode(&query), language
            )
        } else {
            format!(
                "https://api.themoviedb.org/3/search/multi?api_key={}&query={}&language={}&include_adult=false",
                api_key, urlencoding::encode(&query), language
            )
        }
    };

    let mut req = client.get(&url).header("accept", "application/json");
    if is_jwt {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let res = req.send().await.map_err(|e| {
        let err = format!("Network error: {}", e);
        log::error!("[TMDB] {}", err);
        err
    })?;

    let status = res.status();
    let json = json_response(res, TMDB_JSON_MAX_BYTES, "tmdb_response_too_large", true).await?;

    // 检查 TMDB API 级别的错误
    if !status.is_success() {
        let err_msg = json
            .get("status_message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown API error");
        let err = format!("TMDB API Error ({}): {}", status, err_msg);
        log::error!("[TMDB] {}", err);
        return Err(err);
    }

    let mut final_json = json;

    // 如果是 IMDb ID，转换结果格式使其与 search/multi 一致
    if is_imdb_id {
        let mut combined_results = Vec::new();
        if let Some(movies) = final_json.get("movie_results").and_then(|r| r.as_array()) {
            for m in movies {
                let mut item = m.clone();
                if item.get("media_type").is_none() {
                    item["media_type"] = serde_json::json!("movie");
                }
                combined_results.push(item);
            }
        }
        if let Some(tvs) = final_json.get("tv_results").and_then(|r| r.as_array()) {
            for t in tvs {
                let mut item = t.clone();
                if item.get("media_type").is_none() {
                    item["media_type"] = serde_json::json!("tv");
                }
                combined_results.push(item);
            }
        }
        if let Some(seasons) = final_json
            .get("tv_season_results")
            .and_then(|r| r.as_array())
        {
            for s in seasons {
                let mut item = s.clone();
                if item.get("media_type").is_none() {
                    item["media_type"] = serde_json::json!("tv_season");
                }
                combined_results.push(item);
            }
        }
        if let Some(episodes) = final_json
            .get("tv_episode_results")
            .and_then(|r| r.as_array())
        {
            for e in episodes {
                let mut item = e.clone();
                if item.get("media_type").is_none() {
                    item["media_type"] = serde_json::json!("tv_episode");
                }
                combined_results.push(item);
            }
        }
        final_json = serde_json::json!({
            "results": combined_results
        });
    }

    // 如果中文没搜到，尝试英文
    if let Some(results) = final_json.get("results").and_then(|r| r.as_array()) {
        if results.is_empty() && language != "en-US" {
            log::info!("[TMDB] No zh-CN results, trying en-US...");
            let en_url = if is_imdb_id {
                if is_jwt {
                    format!("https://api.themoviedb.org/3/find/{}?external_source=imdb_id&language=en-US", query)
                } else {
                    format!("https://api.themoviedb.org/3/find/{}?api_key={}&external_source=imdb_id&language=en-US", query, api_key)
                }
            } else {
                if is_jwt {
                    format!(
                         "https://api.themoviedb.org/3/search/multi?query={}&language=en-US&include_adult=false",
                         urlencoding::encode(&query)
                     )
                } else {
                    format!(
                         "https://api.themoviedb.org/3/search/multi?api_key={}&query={}&language=en-US&include_adult=false",
                         api_key, urlencoding::encode(&query)
                     )
                }
            };
            let mut en_req = client.get(&en_url).header("accept", "application/json");
            if is_jwt {
                en_req = en_req.header("Authorization", format!("Bearer {}", api_key));
            }
            let en_res = en_req.send().await.map_err(|e| e.to_string())?;
            let en_status = en_res.status();
            let mut en_json =
                json_response(en_res, TMDB_JSON_MAX_BYTES, "tmdb_response_too_large", true).await?;
            if !en_status.is_success() {
                let message = en_json
                    .get("status_message")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown API error");
                return Err(format!("TMDB API Error ({}): {}", en_status, message));
            }

            if is_imdb_id {
                let mut combined_results = Vec::new();
                if let Some(movies) = en_json.get("movie_results").and_then(|r| r.as_array()) {
                    for m in movies {
                        let mut item = m.clone();
                        if item.get("media_type").is_none() {
                            item["media_type"] = serde_json::json!("movie");
                        }
                        combined_results.push(item);
                    }
                }
                if let Some(tvs) = en_json.get("tv_results").and_then(|r| r.as_array()) {
                    for t in tvs {
                        let mut item = t.clone();
                        if item.get("media_type").is_none() {
                            item["media_type"] = serde_json::json!("tv");
                        }
                        combined_results.push(item);
                    }
                }
                if let Some(seasons) = en_json.get("tv_season_results").and_then(|r| r.as_array()) {
                    for s in seasons {
                        let mut item = s.clone();
                        if item.get("media_type").is_none() {
                            item["media_type"] = serde_json::json!("tv_season");
                        }
                        combined_results.push(item);
                    }
                }
                if let Some(episodes) = en_json.get("tv_episode_results").and_then(|r| r.as_array())
                {
                    for e in episodes {
                        let mut item = e.clone();
                        if item.get("media_type").is_none() {
                            item["media_type"] = serde_json::json!("tv_episode");
                        }
                        combined_results.push(item);
                    }
                }
                en_json = serde_json::json!({
                    "results": combined_results
                });
            }

            return Ok(en_json);
        }
        log::info!("[TMDB] Found {} results", results.len());
    }

    Ok(final_json)
}

pub async fn get_tmdb_detail(
    api_key: String,
    id: i32,
    media_type: String,
    language: String,
    proxy: Option<String>,
) -> Result<Value, String> {
    if !matches!(media_type.as_str(), "movie" | "tv" | "collection") {
        return Err("Unsupported TMDB media type".to_string());
    }
    if id <= 0 {
        return Err("Invalid TMDB ID".to_string());
    }
    log::info!("[TMDB] Fetching detail for {} ID: {}", media_type, id);

    let builder = client_builder(proxy, Duration::from_secs(30))?;
    let client = builder.build().map_err(|e| e.to_string())?;

    let is_jwt = api_key.contains('.') || api_key.len() > 40;

    let append = if media_type == "collection" {
        ""
    } else {
        "&append_to_response=external_ids"
    };
    let url = if is_jwt {
        format!(
            "https://api.themoviedb.org/3/{}/{}?language={}{}",
            media_type, id, language, append
        )
    } else {
        format!(
            "https://api.themoviedb.org/3/{}/{}?api_key={}&language={}{}",
            media_type, id, api_key, language, append
        )
    };

    let mut req = client.get(&url).header("accept", "application/json");
    if is_jwt {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let res = req.send().await.map_err(|e| {
        log::error!("[TMDB] Detail network error: {}", e);
        e.to_string()
    })?;
    let status = res.status();
    let json = json_response(res, TMDB_JSON_MAX_BYTES, "tmdb_response_too_large", true).await?;
    if !status.is_success() {
        let message = json
            .get("status_message")
            .and_then(Value::as_str)
            .unwrap_or("Unknown API error");
        return Err(format!("TMDB API Error ({}): {}", status, message));
    }
    Ok(json)
}

pub async fn get_tmdb_season_detail(
    api_key: String,
    series_id: i32,
    season_number: i32,
    language: String,
    proxy: Option<String>,
) -> Result<Value, String> {
    if series_id <= 0 || season_number < 0 {
        return Err("Invalid TMDB TV season identity".to_string());
    }
    log::info!(
        "[TMDB] Fetching TV season detail for series {} season {}",
        series_id,
        season_number
    );

    let builder = client_builder(proxy, Duration::from_secs(30))?;
    let client = builder.build().map_err(|e| e.to_string())?;
    let is_jwt = api_key.contains('.') || api_key.len() > 40;
    let url = if is_jwt {
        format!(
            "https://api.themoviedb.org/3/tv/{}/season/{}?language={}",
            series_id, season_number, language
        )
    } else {
        format!(
            "https://api.themoviedb.org/3/tv/{}/season/{}?api_key={}&language={}",
            series_id, season_number, api_key, language
        )
    };

    let mut req = client.get(&url).header("accept", "application/json");
    if is_jwt {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }
    let res = req.send().await.map_err(|e| {
        log::error!("[TMDB] TV season detail network error: {}", e);
        e.to_string()
    })?;
    let status = res.status();
    let json = json_response(res, TMDB_JSON_MAX_BYTES, "tmdb_response_too_large", true).await?;
    if !status.is_success() {
        let message = json
            .get("status_message")
            .and_then(Value::as_str)
            .unwrap_or("Unknown API error");
        return Err(format!("TMDB API Error ({}): {}", status, message));
    }
    Ok(json)
}

pub async fn download_poster(
    paths: &AppPaths,
    state: &PosterDownloadState,
    poster_path: String,
    size: Option<String>,
    proxy: Option<String>,
) -> Result<PosterDownloadResult, String> {
    let size = size.unwrap_or_else(|| "w342".to_string());
    let file_name = poster_cache::normalized_file_name(&poster_path, &size)?;

    let local_path = paths.poster_file(&file_name)?;
    if poster_cache::read_valid(&local_path).is_ok() {
        return Ok(PosterDownloadResult {
            status: "cache_hit".to_string(),
            file_name,
        });
    }

    let file_lock = state.file_lock(&file_name).await;
    let _file_guard = file_lock.lock().await;
    if poster_cache::read_valid(&local_path).is_ok() {
        return Ok(PosterDownloadResult {
            status: "cache_hit".to_string(),
            file_name,
        });
    }
    let _permit = state
        .permits
        .acquire()
        .await
        .map_err(|_| "poster_download_unavailable".to_string())?;

    let url = format!("https://image.tmdb.org/t/p/{size}{poster_path}");
    let builder =
        client_builder(proxy, Duration::from_secs(45))?.redirect(reqwest::redirect::Policy::none());
    let client = builder.build().map_err(|e| e.to_string())?;
    let mut res = client.get(url).send().await.map_err(|error| {
        if error.is_timeout() {
            "poster_timeout".to_string()
        } else {
            "poster_network_failed".to_string()
        }
    })?;

    if !res.status().is_success() {
        return Err(format!("poster_http_status:{}", res.status().as_u16()));
    }
    if res
        .content_length()
        .is_some_and(|length| length > poster_cache::POSTER_MAX_BYTES)
    {
        return Err("poster_response_too_large".to_string());
    }
    if let Some(content_type) = res.headers().get(reqwest::header::CONTENT_TYPE) {
        let value = content_type
            .to_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !value.starts_with("image/") && !value.contains("octet-stream") {
            return Err("poster_content_type_invalid".to_string());
        }
    }

    let temp_path = paths
        .posters()
        .join(format!(".poster-{}.part", Uuid::new_v4()));
    let result = async {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|_| "poster_cache_write_failed".to_string())?;
        let mut written = 0_u64;
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|_| "poster_network_failed".to_string())?
        {
            written = written.saturating_add(chunk.len() as u64);
            if written > poster_cache::POSTER_MAX_BYTES {
                return Err("poster_response_too_large".to_string());
            }
            file.write_all(&chunk)
                .map_err(|_| "poster_cache_write_failed".to_string())?;
        }
        file.flush()
            .map_err(|_| "poster_cache_write_failed".to_string())?;
        file.sync_all()
            .map_err(|_| "poster_cache_write_failed".to_string())?;
        drop(file);
        poster_cache::read_valid(&temp_path)?;
        if local_path.exists() {
            fs::remove_file(&local_path).map_err(|_| "poster_cache_write_failed".to_string())?;
        }
        fs::rename(&temp_path, &local_path).map_err(|_| "poster_cache_write_failed".to_string())?;
        Ok::<(), String>(())
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result?;
    Ok(PosterDownloadResult {
        status: "downloaded".to_string(),
        file_name,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavResponse {
    pub status: u16,
    pub body: Option<Value>,
    pub etag: Option<String>,
    pub text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavRequest {
    pub method: String,
    pub url: String,
    pub username: String,
    pub password: String,
    pub body: Option<String>,
    pub proxy: Option<String>,
    pub if_match: Option<String>,
    pub if_none_match: Option<String>,
    pub if_dav_etag: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredWebDavRequest {
    pub target_id: String,
    pub target_epoch: u64,
    pub method: String,
    pub url: String,
    pub body: Option<String>,
    pub proxy: Option<String>,
    pub if_match: Option<String>,
    pub if_none_match: Option<String>,
    pub if_dav_etag: Option<String>,
}

fn etag_shape(value: Option<&str>) -> &'static str {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return "missing";
    };
    if value.starts_with("W/\"") && value.ends_with('"') {
        "weak"
    } else if value.starts_with('"') && value.ends_with('"') {
        "strong"
    } else if !value.contains(['\r', '\n', '"']) {
        "unquoted"
    } else {
        "invalid"
    }
}

fn validator_fingerprint(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Keep credentials, query tokens, and fragments out of diagnostic logs while
/// retaining the origin/path needed to identify a failing WebDAV endpoint.
fn safe_url_for_log(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return "<invalid-url>".to_string();
    };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    parsed.to_string()
}

pub async fn webdav_request(request: WebDavRequest) -> Result<WebDavResponse, String> {
    log::info!(
        "[WebDAV] {} Request to: {}",
        request.method,
        safe_url_for_log(&request.url)
    );
    if request.method == "PUT" {
        if let Some(value) = request.if_match.as_deref() {
            log::info!(
                "[WebDAV] PUT condition: if-match; validator fingerprint: {}",
                validator_fingerprint(value)
            );
        } else if let Some(value) = request.if_dav_etag.as_deref() {
            log::info!(
                "[WebDAV] PUT condition: dav-if; validator fingerprint: {}",
                validator_fingerprint(value)
            );
        } else if let Some(value) = request.if_none_match.as_deref() {
            log::info!(
                "[WebDAV] PUT condition: if-none-match; validator fingerprint: {}",
                validator_fingerprint(value)
            );
        }
    }

    if request.method == "PUT"
        && request
            .body
            .as_ref()
            .is_some_and(|body| body.len() > WEBDAV_JSON_MAX_BYTES)
    {
        return Err("webdav_request_too_large".to_string());
    }
    let builder = client_builder(request.proxy, Duration::from_secs(120))?;
    let client = builder.build().map_err(|e| e.to_string())?;

    let mut req_builder = match request.method.as_str() {
        "MKCOL" => client.request(reqwest::Method::from_bytes(b"MKCOL").unwrap(), &request.url),
        "PROPFIND" => client.request(
            reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
            &request.url,
        ),
        "PUT" => client.put(&request.url),
        "GET" => client.get(&request.url),
        _ => return Err(format!("Unsupported method: {}", request.method)),
    };

    req_builder = req_builder.basic_auth(request.username, Some(request.password));

    if request.method == "PROPFIND" {
        req_builder = req_builder
            .header("Depth", "0")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(
                r#"<?xml version="1.0" encoding="utf-8" ?><d:propfind xmlns:d="DAV:"><d:prop><d:getetag /></d:prop></d:propfind>"#,
            );
    }

    if let Some(value) = request.if_match {
        req_builder = req_builder.header(reqwest::header::IF_MATCH, value);
    }
    if let Some(value) = request.if_none_match {
        req_builder = req_builder.header(reqwest::header::IF_NONE_MATCH, value);
    }
    if let Some(value) = request.if_dav_etag {
        req_builder = req_builder.header("If", format!("([{value}])"));
    }

    if request.method != "PROPFIND" {
        if let Some(b) = request.body {
            req_builder = req_builder
                .header("Content-Type", "application/json")
                .body(b);
        }
    }

    let res = req_builder.send().await.map_err(|_e| {
        log::error!(
            "[WebDAV] {} request failed for {}; transport error redacted",
            request.method,
            safe_url_for_log(&request.url)
        );
        "webdav_transport_error".to_string()
    })?;

    let status = res.status();
    let etag = res
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    if request.method == "GET" && status.is_success() {
        log::info!(
            "[WebDAV] GET completed with status {}; ETag shape: {}",
            status.as_u16(),
            etag_shape(etag.as_deref())
        );
        let json = json_response(
            res,
            WEBDAV_JSON_MAX_BYTES,
            "webdav_response_too_large",
            false,
        )
        .await?;
        return Ok(WebDavResponse {
            status: status.as_u16(),
            body: Some(json),
            etag,
            text: None,
        });
    }

    if request.method == "PROPFIND" && status.is_success() {
        let bytes = read_limited(res, WEBDAV_XML_MAX_BYTES, "webdav_response_too_large").await?;
        let text = String::from_utf8(bytes).map_err(|_| "webdav_xml_invalid".to_string())?;
        log::info!(
            "[WebDAV] PROPFIND completed with status {} and response body",
            status.as_u16()
        );
        return Ok(WebDavResponse {
            status: status.as_u16(),
            body: None,
            etag,
            text: Some(text),
        });
    }

    log::info!(
        "[WebDAV] {} completed with status {}; ETag shape: {}",
        request.method,
        status.as_u16(),
        etag_shape(etag.as_deref())
    );

    Ok(WebDavResponse {
        status: status.as_u16(),
        body: None,
        etag,
        text: None,
    })
}

#[cfg(test)]
mod url_log_tests {
    use super::safe_url_for_log;

    #[test]
    fn redacts_userinfo_query_and_fragment() {
        let safe = safe_url_for_log(
            "https://alice:super-secret@example.test/library?token=do-not-log#secret",
        );
        assert_eq!(safe, "https://example.test/library");
        assert!(!safe.contains("super-secret"));
        assert!(!safe.contains("do-not-log"));
    }

    #[test]
    fn invalid_urls_have_constant_log_value() {
        assert_eq!(safe_url_for_log("not a URL?token=secret"), "<invalid-url>");
    }
}
