//! Android-capable WebDAV transport for frozen S2 immutable objects.
//!
//! This is intentionally an async transport capability only.  It is not
//! registered with the S1 coordinator and it does not perform discovery or
//! publication orchestration.

use async_trait::async_trait;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesDecl, Event};
use quick_xml::name::ResolveResult;
use quick_xml::NsReader;
use reqwest::{Method, StatusCode, Url};
use sha2::{Digest, Sha256};
use std::time::Duration;

use super::canonical::sha256_hex;
use super::immutable_publish::SEGMENT_NAME_WIDTH_V1;
use super::remote_discovery::{
    parse_activation_candidate_path_v1, parse_writer_candidate_path_v1, ObservedCandidateV1,
};

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_PROPFIND_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebDavRootV1 {
    pub canonical_url: String,
    pub normalized_account: String,
    pub physical_root_id: String,
}

#[derive(Clone, Debug)]
pub struct WebDavS2ConfigV1 {
    pub root: WebDavRootV1,
    pub username: String,
    pub password: String,
    pub proxy: Option<String>,
    pub timeout: Duration,
}

/// Frozen root identity.  It intentionally has no `sync-s2-v1` prefix.
pub fn webdav_root_v1(target_url: &str, username: &str) -> Result<WebDavRootV1, &'static str> {
    let mut url = Url::parse(target_url).map_err(|_| "invalid_webdav_root")?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid_webdav_root");
    }
    url.set_fragment(None);
    url.set_query(None);
    let path = url.path().trim_end_matches('/');
    url.set_path(&format!("{path}/"));
    let account = username.trim().to_owned();
    if account.is_empty() {
        return Err("invalid_webdav_account");
    }
    let canonical_url = url.to_string();
    let mut hasher = Sha256::new();
    hasher.update(canonical_url.as_bytes());
    hasher.update([0]);
    hasher.update(account.as_bytes());
    Ok(WebDavRootV1 {
        canonical_url,
        normalized_account: account,
        physical_root_id: format!("s2-root-v1:{:x}", hasher.finalize()),
    })
}

fn safe_relative_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains(['\\', '?', '#', '%'])
        || path.contains("//")
        || path.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
    {
        return Err("invalid_s2_relative_path");
    }
    Ok(())
}

fn child_url(root: &WebDavRootV1, path: &str) -> Result<Url, &'static str> {
    safe_relative_path(path)?;
    Url::parse(&root.canonical_url)
        .map_err(|_| "invalid_webdav_root")?
        .join(path)
        .map_err(|_| "invalid_s2_relative_path")
}

fn valid_immutable_object_path(path: &str) -> bool {
    parse_activation_candidate_path_v1(path).is_some()
        || parse_writer_candidate_path_v1(path).is_some()
}

fn immutable_path_content_hash(path: &str) -> Option<String> {
    match parse_activation_candidate_path_v1(path)
        .or_else(|| parse_writer_candidate_path_v1(path))?
    {
        ObservedCandidateV1::Activation { content_hash, .. }
        | ObservedCandidateV1::Commit { content_hash, .. } => Some(content_hash),
    }
}

fn valid_segment_name(segment: &str) -> bool {
    segment.len() == SEGMENT_NAME_WIDTH_V1
        && segment
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_discovery_directory(path: &str) -> bool {
    if matches!(path, "activations" | "writers") {
        return true;
    }
    let p = path.split('/').collect::<Vec<_>>();
    match p.as_slice() {
        ["writers", id, "segments"] => super::canonical::validate_canonical_uuid_v4(id).is_ok(),
        ["writers", id, "segments", segment] => {
            super::canonical::validate_canonical_uuid_v4(id).is_ok() && valid_segment_name(segment)
        }
        _ => false,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebDavResponseV1 {
    pub status: u16,
    pub body: Vec<u8>,
}

#[async_trait]
pub trait WebDavTransportV1: Send {
    async fn request(
        &mut self,
        method: Method,
        url: Url,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<WebDavResponseV1, ()>;
}

/// Production HTTP implementation.  It is Rustls-only through Android's
/// existing reqwest dependency and does not share S1's JSON decoding boundary.
pub struct ReqwestWebDavTransportV1 {
    client: reqwest::Client,
    username: String,
    password: String,
}

impl ReqwestWebDavTransportV1 {
    pub fn new(config: &WebDavS2ConfigV1) -> Result<Self, &'static str> {
        let mut builder = reqwest::Client::builder()
            .timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(proxy) = config
            .proxy
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|_| "invalid_proxy")?);
        }
        Ok(Self {
            client: builder.build().map_err(|_| "webdav_client_failure")?,
            username: config.username.clone(),
            password: config.password.clone(),
        })
    }
}

#[async_trait]
impl WebDavTransportV1 for ReqwestWebDavTransportV1 {
    async fn request(
        &mut self,
        method: Method,
        url: Url,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<WebDavResponseV1, ()> {
        let mut request = self
            .client
            .request(method, url)
            .basic_auth(&self.username, Some(&self.password));
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if let Some(bytes) = body {
            request = request.body(bytes);
        }
        let response = request.send().await.map_err(|_| ())?;
        let limit = if response.status() == StatusCode::MULTI_STATUS {
            MAX_PROPFIND_BYTES
        } else {
            MAX_BODY_BYTES
        };
        if response
            .content_length()
            .is_some_and(|n| n as usize > limit)
        {
            return Err(());
        }
        let mut bytes = Vec::new();
        let mut response = response;
        while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(WebDavResponseV1 {
            status: response.status().as_u16(),
            body: bytes,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebDavGetResultV1 {
    DefinitelyPresent(Vec<u8>),
    DefinitelyAbsent,
    AuthOrCapabilityFailure,
    Indeterminate,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImmutablePutResultV1 {
    Published,
    AlreadyPresentExact,
    CorruptionMismatch,
    AuthOrCapabilityFailure,
    Indeterminate,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectoryListResultV1 {
    Entries(Vec<String>),
    AuthOrCapabilityFailure,
    Indeterminate,
}

pub struct WebDavS2AdapterV1<T> {
    root: WebDavRootV1,
    transport: T,
}

impl<T: WebDavTransportV1> WebDavS2AdapterV1<T> {
    pub fn new(config: WebDavS2ConfigV1, transport: T) -> Result<Self, &'static str> {
        let account = config.username.trim().to_owned();
        if config.root != webdav_root_v1(&config.root.canonical_url, &account)? {
            return Err("webdav_root_credentials_mismatch");
        }
        Ok(Self {
            root: config.root,
            transport,
        })
    }

    async fn request(
        &mut self,
        method: Method,
        path: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<WebDavResponseV1, ()> {
        self.transport
            .request(
                method,
                child_url(&self.root, path).map_err(|_| ())?,
                headers,
                body,
            )
            .await
    }

    pub async fn get_exact(&mut self, path: &str) -> WebDavGetResultV1 {
        if !valid_immutable_object_path(path) {
            return WebDavGetResultV1::Indeterminate;
        }
        match self.request(Method::GET, path, Vec::new(), None).await {
            Ok(response) if (200..300).contains(&response.status) => {
                WebDavGetResultV1::DefinitelyPresent(response.body)
            }
            Ok(WebDavResponseV1 { status: 404, .. }) => WebDavGetResultV1::DefinitelyAbsent,
            Ok(WebDavResponseV1 {
                status: 401 | 403, ..
            }) => WebDavGetResultV1::AuthOrCapabilityFailure,
            _ => WebDavGetResultV1::Indeterminate,
        }
    }

    async fn ensure_collection(&mut self, path: &str) -> Result<(), ImmutablePutResultV1> {
        match self
            .request(
                Method::from_bytes(b"MKCOL").expect("method"),
                path,
                Vec::new(),
                None,
            )
            .await
        {
            Ok(WebDavResponseV1 { status: 201, .. }) => Ok(()),
            Ok(WebDavResponseV1 {
                status: 401 | 403, ..
            }) => Err(ImmutablePutResultV1::AuthOrCapabilityFailure),
            // 405 can mean an existing collection only.  Verify that exact fact.
            Ok(WebDavResponseV1 { status: 405, .. }) => match self
                .request(
                    Method::from_bytes(b"PROPFIND").expect("method"),
                    path,
                    vec![("Depth".into(), "0".into())],
                    None,
                )
                .await
            {
                Ok(WebDavResponseV1 { status: 207, body })
                    if depth_zero_is_collection(&self.root, path, &body) =>
                {
                    Ok(())
                }
                Ok(WebDavResponseV1 {
                    status: 401 | 403, ..
                }) => Err(ImmutablePutResultV1::AuthOrCapabilityFailure),
                _ => Err(ImmutablePutResultV1::Indeterminate),
            },
            _ => Err(ImmutablePutResultV1::Indeterminate),
        }
    }

    async fn ensure_parents(&mut self, path: &str) -> Result<(), ImmutablePutResultV1> {
        let parents = match path.split('/').collect::<Vec<_>>().as_slice() {
            ["activations", _] => vec!["activations".to_owned()],
            ["writers", writer, "segments", segment, _] => vec![
                "writers".into(),
                format!("writers/{writer}"),
                format!("writers/{writer}/segments"),
                format!("writers/{writer}/segments/{segment}"),
            ],
            _ => return Err(ImmutablePutResultV1::Indeterminate),
        };
        for parent in parents {
            self.ensure_collection(&parent).await?;
        }
        Ok(())
    }

    /// Defends immutable identity before and after PUT.  Provider conditional
    /// support is not trusted for correctness: every ambiguous outcome is
    /// settled with an exact GET of the already prepared bytes.
    pub async fn put_immutable(
        &mut self,
        path: &str,
        exact_bytes: &[u8],
        content_hash: &str,
    ) -> ImmutablePutResultV1 {
        if !valid_immutable_object_path(path)
            || sha256_hex(exact_bytes) != content_hash
            || immutable_path_content_hash(path).as_deref() != Some(content_hash)
        {
            return ImmutablePutResultV1::CorruptionMismatch;
        }
        // The frozen ordering deliberately admits the canonical parent chain
        // before the decisive object preflight.  A pre-provision 404 is never
        // evidence that it remains safe to PUT after another writer creates
        // the object while the hierarchy is being admitted.
        if let Err(result) = self.ensure_parents(path).await {
            return result;
        }
        match self.get_exact(path).await {
            WebDavGetResultV1::DefinitelyPresent(existing) => {
                return if existing == exact_bytes {
                    ImmutablePutResultV1::AlreadyPresentExact
                } else {
                    ImmutablePutResultV1::CorruptionMismatch
                }
            }
            WebDavGetResultV1::AuthOrCapabilityFailure => {
                return ImmutablePutResultV1::AuthOrCapabilityFailure
            }
            WebDavGetResultV1::Indeterminate => return ImmutablePutResultV1::Indeterminate,
            WebDavGetResultV1::DefinitelyAbsent => {}
        }
        let put = self
            .request(
                Method::PUT,
                path,
                vec![("If-None-Match".into(), "*".into())],
                Some(exact_bytes.to_vec()),
            )
            .await;
        if matches!(
            put,
            Ok(WebDavResponseV1 {
                status: 401 | 403,
                ..
            })
        ) {
            return ImmutablePutResultV1::AuthOrCapabilityFailure;
        }
        match self.get_exact(path).await {
            WebDavGetResultV1::DefinitelyPresent(existing) if existing == exact_bytes => {
                ImmutablePutResultV1::Published
            }
            WebDavGetResultV1::DefinitelyPresent(_) => ImmutablePutResultV1::CorruptionMismatch,
            WebDavGetResultV1::AuthOrCapabilityFailure => {
                ImmutablePutResultV1::AuthOrCapabilityFailure
            }
            _ => ImmutablePutResultV1::Indeterminate,
        }
    }

    pub async fn list_directory(&mut self, path: &str) -> DirectoryListResultV1 {
        let directory = path.trim_end_matches('/');
        if !valid_discovery_directory(directory) {
            return DirectoryListResultV1::Indeterminate;
        }
        let directory_url = match Url::parse(&self.root.canonical_url)
            .and_then(|root| root.join(&format!("{directory}/")))
        {
            Ok(url) => url,
            Err(_) => return DirectoryListResultV1::Indeterminate,
        };
        let response = self
            .transport
            .request(
                Method::from_bytes(b"PROPFIND").expect("method"),
                directory_url,
                vec![("Depth".into(), "1".into())],
                None,
            )
            .await;
        let Ok(response) = response else {
            return DirectoryListResultV1::Indeterminate;
        };
        if matches!(response.status, 401 | 403) {
            return DirectoryListResultV1::AuthOrCapabilityFailure;
        }
        if response.status != 207 {
            return DirectoryListResultV1::Indeterminate;
        }
        parse_listing(&self.root, directory, &response.body)
            .map(DirectoryListResultV1::Entries)
            .unwrap_or(DirectoryListResultV1::Indeterminate)
    }
}

#[derive(Default)]
struct DavResponseV1 {
    href: Option<String>,
    collection: bool,
    response_status: Option<bool>,
}

fn is_xml_s(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

fn validate_xml_declaration(declaration: &BytesDecl<'_>) -> bool {
    let bytes: &[u8] = declaration.as_ref();
    if bytes.len() <= 3 || !bytes.starts_with(b"xml") || !is_xml_s(bytes[3]) {
        return false;
    }
    let mut cursor = 3;
    let mut last_order = 0;
    let mut count = 0;
    while cursor < bytes.len() {
        let whitespace = cursor;
        while cursor < bytes.len() && is_xml_s(bytes[cursor]) {
            cursor += 1;
        }
        if cursor == bytes.len() {
            return count > 0;
        }
        if count > 0 && cursor == whitespace {
            return false;
        }
        let key_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
            cursor += 1;
        }
        if key_start == cursor {
            return false;
        }
        let key = &bytes[key_start..cursor];
        while cursor < bytes.len() && is_xml_s(bytes[cursor]) {
            cursor += 1;
        }
        if cursor == bytes.len() || bytes[cursor] != b'=' {
            return false;
        }
        cursor += 1;
        while cursor < bytes.len() && is_xml_s(bytes[cursor]) {
            cursor += 1;
        }
        if cursor == bytes.len() || !matches!(bytes[cursor], b'\'' | b'\"') {
            return false;
        }
        let quote = bytes[cursor];
        cursor += 1;
        let value_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != quote {
            cursor += 1;
        }
        if cursor == bytes.len() {
            return false;
        }
        let value = &bytes[value_start..cursor];
        cursor += 1;
        let (order, accepted) = match key {
            b"version" => (1, value == b"1.0"),
            b"encoding" => (
                2,
                value.eq_ignore_ascii_case(b"utf-8") || value.eq_ignore_ascii_case(b"utf8"),
            ),
            b"standalone" => (3, matches!(value, b"yes" | b"no")),
            _ => return false,
        };
        if !accepted || order <= last_order || (last_order == 0 && order != 1) {
            return false;
        }
        last_order = order;
        count += 1;
    }
    count > 0 && last_order >= 1
}

fn success_status(value: &str) -> bool {
    let mut fields = value.split_ascii_whitespace();
    matches!((fields.next(), fields.next()), (Some(version), Some(code)) if version.starts_with("HTTP/") && matches!(code.parse::<u16>(), Ok(200..=299)))
}

fn decode_segment(segment: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let raw = segment.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] != b'%' {
            bytes.push(raw[index]);
            index += 1;
            continue;
        }
        let value = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        };
        bytes.push((value(*raw.get(index + 1)?)? << 4) | value(*raw.get(index + 2)?)?);
        index += 3;
    }
    let result = String::from_utf8(bytes).ok()?;
    (!result.is_empty() && !matches!(result.as_str(), "." | "..") && !result.contains(['/', '\\']))
        .then_some(result)
}

fn raw_href_is_safe(href: &str) -> bool {
    if href.contains('\\') {
        return false;
    }
    let before_suffix = &href[..href.find(['?', '#']).unwrap_or(href.len())];
    let path = if before_suffix.starts_with('/') {
        before_suffix
    } else if let Some(scheme) = before_suffix.find("://") {
        let after_authority = &before_suffix[scheme + 3..];
        after_authority
            .find('/')
            .map(|index| &after_authority[index..])
            .unwrap_or("")
    } else {
        before_suffix
    };
    let absolute = path.starts_with('/');
    let path = path.strip_prefix('/').unwrap_or(path);
    if absolute && path.starts_with('/') {
        return false;
    }
    if path.is_empty() {
        return true;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    let trailing = parts
        .iter()
        .rev()
        .take_while(|part| part.is_empty())
        .count();
    trailing <= 1
        && parts[..parts.len().saturating_sub(trailing)]
            .iter()
            .all(|part| decode_segment(part).is_some())
}

fn decoded_segments(url: &Url) -> Option<(Vec<String>, usize)> {
    let parts = url.path().strip_prefix('/')?.split('/').collect::<Vec<_>>();
    let trailing = parts
        .iter()
        .rev()
        .take_while(|part| part.is_empty())
        .count();
    let parts = &parts[..parts.len().saturating_sub(trailing)];
    (!parts.iter().any(|part| part.is_empty()))
        .then(|| {
            (
                parts
                    .iter()
                    .map(|part| decode_segment(part))
                    .collect::<Option<Vec<_>>>(),
                trailing,
            )
        })
        .and_then(|(parts, trailing)| Some((parts?, trailing)))
}

fn directory_child(
    root: &WebDavRootV1,
    directory: &str,
    directory_url: &Url,
    href: &str,
) -> Option<Option<String>> {
    if !raw_href_is_safe(href) {
        return None;
    }
    let root_url = Url::parse(&root.canonical_url).ok()?;
    let observed = directory_url.join(href).ok()?;
    if observed.origin() != root_url.origin()
        || observed.query().is_some()
        || observed.fragment().is_some()
    {
        return None;
    }
    let (root_segments, root_trailing) = decoded_segments(&root_url)?;
    let (observed_segments, trailing) = decoded_segments(&observed)?;
    if root_trailing != 1 || trailing > 1 || !observed_segments.starts_with(&root_segments) {
        return None;
    }
    let directory_segments = directory.split('/').map(str::to_owned).collect::<Vec<_>>();
    let expected = root_segments.len() + directory_segments.len();
    if observed_segments.len() < expected
        || observed_segments[root_segments.len()..expected] != directory_segments
    {
        return None;
    }
    match observed_segments.len() - expected {
        0 => Some(None),
        1 => {
            let child = observed_segments.last()?.clone();
            safe_relative_path(&child).ok()?;
            Some(Some(child))
        }
        _ => None,
    }
}

/// Narrow namespace-aware DAV parser shared by collection proof and listing.
/// It admits only one complete DAV multistatus document and never treats a
/// status code alone, an arbitrary XML document, or an incomplete response as
/// authoritative evidence.
fn parse_dav_responses(xml: &[u8]) -> Option<Vec<DavResponseV1>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<(Vec<u8>, bool)>::new();
    let mut responses = Vec::new();
    let mut current = None::<DavResponseV1>;
    // (element name, whether it is the propstat status, captured text)
    let mut capture = None::<(Vec<u8>, bool, String)>;
    let mut propstat = None::<(bool, Option<bool>)>;
    let mut root_seen = false;
    let mut root_closed = false;
    let mut declaration_seen = false;
    let mut prolog_consumed = false;
    loop {
        match reader.read_resolved_event() {
            Ok((namespace, Event::Start(event))) => {
                if root_closed {
                    return None;
                }
                let dav =
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == b"DAV:");
                let local = event.local_name().as_ref().to_vec();
                let depth = stack.len();
                if depth == 0 && (!dav || local != b"multistatus" || root_seen) {
                    return None;
                }
                if depth == 1 && (!dav || local != b"response" || current.is_some()) {
                    return None;
                }
                if depth == 1 {
                    current = Some(DavResponseV1::default());
                }
                if depth == 2 && dav && (local == b"href" || local == b"status") {
                    if capture.is_some() {
                        return None;
                    }
                    capture = Some((local.clone(), false, String::new()));
                }
                if depth == 2 && dav && local == b"propstat" {
                    if propstat.is_some() {
                        return None;
                    }
                    propstat = Some((false, None));
                }
                if depth == 3 && dav && local == b"status" && propstat.is_some() {
                    if capture.is_some() {
                        return None;
                    }
                    capture = Some((local.clone(), true, String::new()));
                }
                if depth == 5
                    && dav
                    && local == b"collection"
                    && matches!(stack.as_slice(), [(a,true),(b,true),(c,true),(d,true),(e,true)] if a == b"multistatus" && b == b"response" && c == b"propstat" && d == b"prop" && e == b"resourcetype")
                {
                    propstat.as_mut()?.0 = true;
                }
                stack.push((local, dav));
                root_seen = true;
            }
            Ok((namespace, Event::Empty(event))) => {
                let dav =
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == b"DAV:");
                let local = event.local_name().as_ref().to_vec();
                if stack.is_empty() {
                    if !dav || local != b"multistatus" || root_seen {
                        return None;
                    }
                    root_seen = true;
                    root_closed = true;
                    continue;
                }
                if stack.len() == 5
                    && dav
                    && local == b"collection"
                    && matches!(stack.as_slice(), [(a,true),(b,true),(c,true),(d,true),(e,true)] if a == b"multistatus" && b == b"response" && c == b"propstat" && d == b"prop" && e == b"resourcetype")
                {
                    propstat.as_mut()?.0 = true;
                }
            }
            Ok((_, Event::Text(event))) => {
                if let Some((_, _, text)) = capture.as_mut() {
                    text.push_str(&unescape(&event.xml_content().ok()?).ok()?);
                } else if stack.iter().any(|(local, dav)| *dav && local == b"prop") {
                    let _ = event.xml_content().ok()?;
                } else {
                    let raw: &[u8] = event.as_ref();
                    if !raw.iter().all(|byte| is_xml_s(*byte)) {
                        return None;
                    }
                    if !root_seen {
                        prolog_consumed = true;
                    }
                }
            }
            Ok((namespace, Event::End(event))) => {
                let dav =
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == b"DAV:");
                let local = event.local_name().as_ref().to_vec();
                let (open, open_dav) = stack.pop()?;
                if open != local || open_dav != dav {
                    return None;
                }
                if local == b"href" {
                    let (_, _, value) = capture.take()?;
                    if value.is_empty() || current.as_mut()?.href.replace(value).is_some() {
                        return None;
                    }
                }
                if local == b"status" {
                    let (_, in_propstat, value) = capture.take()?;
                    let successful = success_status(&value);
                    if !successful {
                        return None;
                    }
                    if in_propstat {
                        if propstat.as_mut()?.1.replace(true).is_some() {
                            return None;
                        }
                    } else if current.as_mut()?.response_status.replace(true).is_some() {
                        return None;
                    }
                }
                if local == b"propstat" {
                    let (collection, status) = propstat.take()?;
                    if status != Some(true) {
                        return None;
                    }
                    if collection {
                        current.as_mut()?.collection = true;
                    }
                }
                if local == b"response" {
                    let response = current.take()?;
                    if response.href.is_none() || propstat.is_some() {
                        return None;
                    }
                    responses.push(response);
                }
                if local == b"multistatus" {
                    if !stack.is_empty() {
                        return None;
                    }
                    root_closed = true;
                }
            }
            Ok((_, Event::Eof)) => {
                return (root_seen
                    && root_closed
                    && stack.is_empty()
                    && capture.is_none()
                    && current.is_none())
                .then_some(responses)
            }
            Ok((_, Event::Decl(declaration))) => {
                if root_seen
                    || declaration_seen
                    || prolog_consumed
                    || !validate_xml_declaration(&declaration)
                {
                    return None;
                }
                declaration_seen = true;
            }
            Ok((_, Event::Comment(_) | Event::PI(_))) => {
                if !root_seen {
                    prolog_consumed = true;
                }
            }
            Ok((_, Event::DocType(_) | Event::CData(_) | Event::GeneralRef(_))) | Err(_) => {
                return None
            }
        }
    }
}

fn depth_zero_is_collection(root: &WebDavRootV1, path: &str, xml: &[u8]) -> bool {
    let directory_url = match child_url(root, path) {
        Ok(value) => value,
        Err(_) => return false,
    };
    parse_dav_responses(xml).is_some_and(|responses| {
        responses.into_iter().any(|response| {
            response.collection
                && response
                    .href
                    .as_deref()
                    .and_then(|href| directory_child(root, path, &directory_url, href))
                    .is_some_and(|child| child.is_none())
        })
    })
}

fn parse_listing(root: &WebDavRootV1, directory: &str, xml: &[u8]) -> Option<Vec<String>> {
    let directory_url = Url::parse(&root.canonical_url)
        .ok()?
        .join(&format!("{directory}/"))
        .ok()?;
    let mut entries = Vec::new();
    for response in parse_dav_responses(xml)? {
        match directory_child(root, directory, &directory_url, response.href.as_deref()?)? {
            None => {}
            Some(child) => entries.push(format!("{directory}/{child}")),
        }
    }
    entries.sort();
    entries.dedup();
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const ACTIVATION: &str = "activations/123e4567-e89b-12d3-a456-426614174000--0000000000000000000000000000000000000000000000000000000000000000.json";
    const WRITER: &str = "writers/123e4567-e89b-42d3-a456-426614174000/segments/00000000000000/00000000000000000001--123e4567-e89b-42d3-a456-426614174001--0000000000000000000000000000000000000000000000000000000000000000.json";
    fn activation_path(hash: &str) -> String {
        format!("activations/123e4567-e89b-12d3-a456-426614174000--{hash}.json")
    }
    fn writer_path(hash: &str) -> String {
        format!("writers/123e4567-e89b-42d3-a456-426614174000/segments/00000000000000/00000000000000000001--123e4567-e89b-42d3-a456-426614174001--{hash}.json")
    }
    type FakeCall = (Method, String, Vec<(String, String)>, Option<Vec<u8>>);
    struct Fake {
        calls: Vec<FakeCall>,
        replies: VecDeque<Result<WebDavResponseV1, ()>>,
    }
    #[async_trait]
    impl WebDavTransportV1 for Fake {
        async fn request(
            &mut self,
            method: Method,
            url: Url,
            headers: Vec<(String, String)>,
            body: Option<Vec<u8>>,
        ) -> Result<WebDavResponseV1, ()> {
            self.calls
                .push((method, url.path().to_owned(), headers, body));
            self.replies.pop_front().expect("reply")
        }
    }
    fn response(status: u16, body: &[u8]) -> Result<WebDavResponseV1, ()> {
        Ok(WebDavResponseV1 {
            status,
            body: body.to_vec(),
        })
    }
    fn adapter(replies: Vec<Result<WebDavResponseV1, ()>>) -> WebDavS2AdapterV1<Fake> {
        let root = webdav_root_v1("https://example.test/dav", "alice").unwrap();
        WebDavS2AdapterV1::new(
            WebDavS2ConfigV1 {
                root,
                username: "alice".into(),
                password: "secret".into(),
                proxy: None,
                timeout: Duration::from_secs(1),
            },
            Fake {
                calls: Vec::new(),
                replies: replies.into(),
            },
        )
        .unwrap()
    }
    fn block<T>(future: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(future)
    }
    fn collection_xml(path: &str) -> Vec<u8> {
        format!(r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/{path}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#).into_bytes()
    }
    fn collection_xml_with_status(path: &str, status: &str) -> Vec<u8> {
        format!(r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/{path}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>{status}</d:status></d:propstat></d:response></d:multistatus>"#).into_bytes()
    }
    #[test]
    fn root_and_frozen_paths_have_no_compatibility_prefix() {
        assert_eq!(
            child_url(
                &webdav_root_v1("https://example.test/dav", "alice").unwrap(),
                ACTIVATION
            )
            .unwrap()
            .path(),
            format!("/dav/{ACTIVATION}")
        );
        assert!(valid_immutable_object_path(WRITER));
    }
    #[test]
    fn exact_get_preserves_raw_bytes_and_classifies_auth_failure() {
        let mut present = adapter(vec![response(200, b"\x00exact\xff")]);
        assert_eq!(
            block(present.get_exact(ACTIVATION)),
            WebDavGetResultV1::DefinitelyPresent(b"\x00exact\xff".to_vec())
        );
        let mut denied = adapter(vec![response(401, b"")]);
        assert_eq!(
            block(denied.get_exact(ACTIVATION)),
            WebDavGetResultV1::AuthOrCapabilityFailure
        );
    }
    #[test]
    fn immutable_first_write_provisions_and_verifies() {
        let bytes = b"exact";
        let hash = sha256_hex(bytes);
        let mut remote = adapter(vec![
            response(201, b""),
            response(201, b""),
            response(201, b""),
            response(201, b""),
            response(404, b""),
            response(201, b""),
            response(200, bytes),
        ]);
        assert_eq!(
            block(remote.put_immutable(&writer_path(&hash), bytes, &hash)),
            ImmutablePutResultV1::Published
        );
        assert!(remote
            .transport
            .calls
            .iter()
            .any(|(m, _, h, _)| *m == Method::PUT
                && h.iter().any(|(n, v)| n == "If-None-Match" && v == "*")));
    }
    #[test]
    fn decisive_preflight_after_parent_provisioning_never_overwrites_a_race() {
        let bytes = b"exact";
        let hash = sha256_hex(bytes);
        let mut remote = adapter(vec![response(201, b""), response(200, b"conflict")]);
        assert_eq!(
            block(remote.put_immutable(&activation_path(&hash), bytes, &hash)),
            ImmutablePutResultV1::CorruptionMismatch
        );
        assert_eq!(
            remote
                .transport
                .calls
                .iter()
                .filter(|(method, ..)| *method == Method::PUT)
                .count(),
            0
        );
    }
    #[test]
    fn exact_retry_is_idempotent_and_mismatch_fails_closed() {
        let bytes = b"exact";
        let hash = sha256_hex(bytes);
        let mut same = adapter(vec![response(201, b""), response(200, bytes)]);
        assert_eq!(
            block(same.put_immutable(&activation_path(&hash), bytes, &hash)),
            ImmutablePutResultV1::AlreadyPresentExact
        );
        let mut different = adapter(vec![response(201, b""), response(200, b"other")]);
        assert_eq!(
            block(different.put_immutable(&activation_path(&hash), bytes, &hash)),
            ImmutablePutResultV1::CorruptionMismatch
        );
    }
    #[test]
    fn lost_put_response_is_verified_by_get() {
        let bytes = b"exact";
        let hash = sha256_hex(bytes);
        let mut remote = adapter(vec![
            response(201, b""),
            response(404, b""),
            Err(()),
            response(200, bytes),
        ]);
        assert_eq!(
            block(remote.put_immutable(&activation_path(&hash), bytes, &hash)),
            ImmutablePutResultV1::Published
        );
    }
    #[test]
    fn if_none_match_unreliability_cannot_weaken_verification() {
        let bytes = b"exact";
        let hash = sha256_hex(bytes);
        let mut remote = adapter(vec![
            response(201, b""),
            response(404, b""),
            response(201, b""),
            response(200, b"other"),
        ]);
        assert_eq!(
            block(remote.put_immutable(&activation_path(&hash), bytes, &hash)),
            ImmutablePutResultV1::CorruptionMismatch
        );
    }
    #[test]
    fn existing_collection_is_verified_with_depth_zero() {
        let mut remote = adapter(vec![
            response(405, b""),
            response(207, &collection_xml("writers")),
        ]);
        assert!(block(remote.ensure_collection("writers")).is_ok());
        assert_eq!(
            remote.transport.calls[1].2,
            vec![("Depth".into(), "0".into())]
        );
    }
    #[test]
    fn depth_zero_requires_successful_exact_dav_collection_property() {
        let root = webdav_root_v1("https://example.test/dav", "alice").unwrap();
        assert!(depth_zero_is_collection(
            &root,
            "writers",
            &collection_xml_with_status("writers", "HTTP/1.1 200 OK")
        ));
        assert!(!depth_zero_is_collection(
            &root,
            "writers",
            &collection_xml_with_status("writers", "HTTP/1.1 404 Not Found")
        ));
        assert!(!depth_zero_is_collection(
            &root,
            "writers",
            &collection_xml("other")
        ));
        assert!(!depth_zero_is_collection(&root, "writers", b"<html/>"));
        assert!(!depth_zero_is_collection(
            &root,
            "writers",
            b"<d:multistatus"
        ));
    }
    #[test]
    fn listing_parses_children_and_rejects_malformed_provider_data() {
        let xml=b"<d:multistatus xmlns:d='DAV:'><d:response><d:href>/dav/activations/</d:href></d:response><d:response><d:href>/dav/activations/a.json</d:href></d:response></d:multistatus>";
        let mut remote = adapter(vec![response(207, xml)]);
        assert_eq!(
            block(remote.list_directory("activations")),
            DirectoryListResultV1::Entries(vec!["activations/a.json".into()])
        );
        let mut bad = adapter(vec![response(207, b"<bad")]);
        assert_eq!(
            block(bad.list_directory("activations")),
            DirectoryListResultV1::Indeterminate
        );
        for invalid in [
            b"".as_slice(),
            b"<html/>".as_slice(),
            b"<d:multistatus".as_slice(),
        ] {
            let mut remote = adapter(vec![response(207, invalid)]);
            assert_eq!(
                block(remote.list_directory("activations")),
                DirectoryListResultV1::Indeterminate
            );
        }
    }
    #[test]
    fn response_and_propstat_statuses_gate_dav_evidence_independently() {
        let root = webdav_root_v1("https://example.test/dav", "alice").unwrap();
        let response_403 = b"<d:multistatus xmlns:d='DAV:'><d:response><d:href>/dav/activations/a.json</d:href><d:status>HTTP/1.1 403 Forbidden</d:status></d:response></d:multistatus>";
        assert_eq!(parse_listing(&root, "activations", response_403), None);
        let missing_propstat_status = b"<d:multistatus xmlns:d='DAV:'><d:response><d:href>/dav/activations/a.json</d:href><d:propstat><d:prop><d:displayname>a</d:displayname></d:prop></d:propstat></d:response></d:multistatus>";
        assert_eq!(
            parse_listing(&root, "activations", missing_propstat_status),
            None
        );
        let failed_response_successful_property = b"<d:multistatus xmlns:d='DAV:'><d:response><d:href>/dav/writers</d:href><d:status>HTTP/1.1 403 Forbidden</d:status><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";
        assert!(!depth_zero_is_collection(
            &root,
            "writers",
            failed_response_successful_property
        ));
    }
    #[test]
    fn declaration_empty_root_and_property_subtrees_match_frozen_dav_rules() {
        let root = webdav_root_v1("https://example.test/dav", "alice").unwrap();
        assert_eq!(
            parse_listing(
                &root,
                "activations",
                b"<?xml version='2.0'?><d:multistatus xmlns:d='DAV:'></d:multistatus>"
            ),
            None
        );
        for valid in [b"<?xml version='1.0'?><d:multistatus xmlns:d='DAV:'></d:multistatus>".as_slice(), b"<?xml version='1.0' encoding='UTF-8' standalone='yes'?><d:multistatus xmlns:d='DAV:'></d:multistatus>".as_slice(), b"<d:multistatus xmlns:d='DAV:'/>".as_slice()] {
            assert_eq!(parse_listing(&root, "activations", valid), Some(Vec::new()));
        }
        let displayname = b"<d:multistatus xmlns:d='DAV:'><d:response><d:href>/dav/activations/</d:href><d:propstat><d:prop><d:displayname>activations</d:displayname></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";
        assert_eq!(
            parse_listing(&root, "activations", displayname),
            Some(Vec::new())
        );
    }
    #[test]
    fn href_resolution_is_directory_bound_and_rejects_lexical_traversal() {
        let root = webdav_root_v1("https://example.test/dav", "alice").unwrap();
        let activations = Url::parse(&root.canonical_url)
            .unwrap()
            .join("activations/")
            .unwrap();
        assert_eq!(
            directory_child(&root, "activations", &activations, "a.json"),
            Some(Some("a.json".into()))
        );
        assert_eq!(
            directory_child(
                &root,
                "activations",
                &activations,
                "/dav/other/../activations/a.json"
            ),
            None
        );
        assert_eq!(
            directory_child(
                &root,
                "activations",
                &activations,
                "/dav/activations/%2e%2e/a.json"
            ),
            None
        );
        assert_eq!(
            directory_child(&root, "activations", &activations, "/dav/writers/a.json"),
            None
        );
        let writers = Url::parse(&root.canonical_url)
            .unwrap()
            .join("writers/")
            .unwrap();
        assert_eq!(
            directory_child(
                &root,
                "writers",
                &writers,
                "/dav/writers/123e4567-e89b-42d3-a456-426614174000/"
            ),
            Some(Some("123e4567-e89b-42d3-a456-426614174000".into()))
        );
    }
    #[test]
    fn adapter_is_not_registered_with_s1_sync() {
        assert!(!include_str!("../commands.rs").contains("s2_lite::webdav_adapter"));
    }
}
