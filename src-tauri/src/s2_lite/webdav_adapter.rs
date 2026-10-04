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

    pub fn physical_root_id(&self) -> &str {
        &self.root.physical_root_id
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

fn is_success_http_status_line(status: &str) -> bool {
    let mut tokens = status.split_ascii_whitespace();
    let Some(version) = tokens.next() else {
        return false;
    };
    let Some(code) = tokens.next() else {
        return false;
    };
    version.starts_with("HTTP/")
        && code.len() == 3
        && code.bytes().all(|byte| byte.is_ascii_digit())
        && matches!(code.parse::<u16>(), Ok(200..=299))
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

fn collection_href_matches(root: &WebDavRootV1, path: &str, href: &str) -> bool {
    // Frozen depth-zero collection proof resolves relative DAV hrefs from the
    // configured WebDAV root.  Listing deliberately uses the requested
    // directory as its base (see `parse_listing` below).
    Url::parse(&root.canonical_url)
        .ok()
        .and_then(|url| directory_child(root, path, &url, href))
        == Some(None)
}

// Ported from WatchTracker-Rust 0c51434e6249a391d6fc1621fe6751e5eba48f27.
// Frozen desktop exact-resource collection proof: unsuccessful unrelated
// propstats contribute no collection evidence; they do not invalidate it.
fn depth_zero_is_collection(root: &WebDavRootV1, path: &str, xml: &[u8]) -> bool {
    #[derive(Clone)]
    struct Element {
        local: Vec<u8>,
        is_dav: bool,
    }
    #[derive(Default)]
    struct Propstat {
        has_collection: bool,
        status_success: Option<bool>,
    }
    #[derive(Default)]
    struct Response {
        href: Option<String>,
        collection: bool,
    }
    enum Capture {
        Href(String),
        ResponseStatus(String),
        PropstatStatus(String),
    }
    #[derive(Eq, PartialEq)]
    enum DocumentPhase {
        Before,
        Inside,
        After,
    }

    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<Element>::new();
    let mut response = None::<Response>;
    let mut propstat = None::<Propstat>;
    let mut capture = None::<Capture>;
    let mut matched = false;
    let mut root_closed = false;
    let mut phase = DocumentPhase::Before;
    let mut declaration_seen = false;
    let mut prolog_consumed = false;

    let start = |namespace: ResolveResult<'_>, local: Vec<u8>, stack: &mut Vec<Element>| {
        let is_dav = matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == b"DAV:");
        stack.push(Element { local, is_dav });
    };

    loop {
        match reader.read_resolved_event() {
            Ok((namespace, Event::Start(event))) => {
                if root_closed || phase == DocumentPhase::After {
                    return false;
                }
                let local = event.local_name().as_ref().to_vec();
                let depth = stack.len();
                let is_dav =
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == b"DAV:");
                if depth == 0 {
                    if phase != DocumentPhase::Before || !is_dav || local != b"multistatus" {
                        return false;
                    }
                    phase = DocumentPhase::Inside;
                }
                if depth == 1 {
                    if !is_dav || local != b"response" || response.is_some() {
                        return false;
                    }
                    response = Some(Response::default());
                } else if response.is_some() {
                    match (depth, local.as_slice()) {
                        (2, b"href") if is_dav && capture.is_none() => {
                            capture = Some(Capture::Href(String::new()));
                        }
                        (2, b"status") if is_dav && capture.is_none() => {
                            capture = Some(Capture::ResponseStatus(String::new()));
                        }
                        (2, b"propstat") if is_dav && propstat.is_none() => {
                            propstat = Some(Propstat::default());
                        }
                        (3, b"status")
                            if is_dav
                                && stack.last().is_some_and(|parent| {
                                    parent.is_dav && parent.local == b"propstat"
                                })
                                && capture.is_none() =>
                        {
                            capture = Some(Capture::PropstatStatus(String::new()));
                        }
                        (5, b"collection")
                            if is_dav
                                && matches!(
                                    stack.as_slice(),
                                    [
                                        Element { local, is_dav: true },
                                        Element { local: response, is_dav: true },
                                        Element { local: propstat, is_dav: true },
                                        Element { local: prop, is_dav: true },
                                        Element { local: resource_type, is_dav: true },
                                    ] if local == b"multistatus"
                                        && response == b"response"
                                        && propstat == b"propstat"
                                        && prop == b"prop"
                                        && resource_type == b"resourcetype"
                                ) =>
                        {
                            let Some(current) = propstat.as_mut() else {
                                return false;
                            };
                            current.has_collection = true;
                        }
                        _ => {}
                    }
                }
                start(namespace, local, &mut stack);
            }
            Ok((namespace, Event::Empty(event))) => {
                if root_closed || phase == DocumentPhase::After {
                    return false;
                }
                let local = event.local_name().as_ref().to_vec();
                let depth = stack.len();
                let is_dav =
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == b"DAV:");
                if depth == 0 {
                    if phase != DocumentPhase::Before || !is_dav || local != b"multistatus" {
                        return false;
                    }
                    phase = DocumentPhase::Inside;
                }
                if depth == 1 {
                    return false;
                }
                if depth == 5
                    && local == b"collection"
                    && is_dav
                    && matches!(
                        stack.as_slice(),
                        [
                            Element { local, is_dav: true },
                            Element { local: response, is_dav: true },
                            Element { local: propstat, is_dav: true },
                            Element { local: prop, is_dav: true },
                            Element { local: resource_type, is_dav: true },
                        ] if local == b"multistatus"
                            && response == b"response"
                            && propstat == b"propstat"
                            && prop == b"prop"
                            && resource_type == b"resourcetype"
                    )
                {
                    let Some(current) = propstat.as_mut() else {
                        return false;
                    };
                    current.has_collection = true;
                }
                start(namespace, local.clone(), &mut stack);
                let Some(closed) = stack.pop() else {
                    return false;
                };
                if closed.local != local || closed.is_dav != is_dav {
                    return false;
                }
                if local == b"multistatus" {
                    root_closed = true;
                    phase = DocumentPhase::After;
                }
            }
            Ok((_, Event::Text(event))) => {
                if let Some(value) = capture.as_mut() {
                    let Ok(decoded) = event.xml10_content() else {
                        return false;
                    };
                    let Ok(text) = unescape(&decoded) else {
                        return false;
                    };
                    match value {
                        Capture::Href(text_out)
                        | Capture::ResponseStatus(text_out)
                        | Capture::PropstatStatus(text_out) => text_out.push_str(&text),
                    }
                } else if stack.is_empty() {
                    let raw: &[u8] = event.as_ref();
                    if !raw.iter().all(|byte| is_xml_s(*byte)) {
                        return false;
                    }
                    if phase == DocumentPhase::Before {
                        prolog_consumed = true;
                    }
                }
            }
            Ok((_, Event::GeneralRef(event))) => {
                if let Some(value) = capture.as_mut() {
                    let Ok(name) = std::str::from_utf8(event.as_ref()) else {
                        return false;
                    };
                    let escaped = format!("&{name};");
                    let Ok(text) = unescape(&escaped) else {
                        return false;
                    };
                    match value {
                        Capture::Href(out)
                        | Capture::ResponseStatus(out)
                        | Capture::PropstatStatus(out) => out.push_str(&text),
                    }
                } else if stack.is_empty() {
                    return false;
                }
            }
            Ok((_, Event::CData(event))) => {
                let Ok(text) = std::str::from_utf8(event.as_ref()) else {
                    return false;
                };
                let Some(value) = capture.as_mut() else {
                    return false;
                };
                match value {
                    Capture::Href(text_out)
                    | Capture::ResponseStatus(text_out)
                    | Capture::PropstatStatus(text_out) => text_out.push_str(text),
                }
            }
            Ok((namespace, Event::End(event))) => {
                let local = event.local_name().as_ref().to_vec();
                let is_dav =
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == b"DAV:");
                let Some(closed) = stack.pop() else {
                    return false;
                };
                if closed.local != local || closed.is_dav != is_dav {
                    return false;
                }
                match local.as_slice() {
                    b"href" => {
                        let Some(Capture::Href(value)) = capture.take() else {
                            return false;
                        };
                        let Some(current) = response.as_mut() else {
                            return false;
                        };
                        if value.is_empty() || current.href.replace(value).is_some() {
                            return false;
                        }
                    }
                    b"status" => match capture.take() {
                        Some(Capture::ResponseStatus(value)) => {
                            if !is_success_http_status_line(&value) {
                                return false;
                            }
                        }
                        Some(Capture::PropstatStatus(value)) => {
                            let Some(current) = propstat.as_mut() else {
                                return false;
                            };
                            if current
                                .status_success
                                .replace(is_success_http_status_line(&value))
                                .is_some()
                            {
                                return false;
                            }
                        }
                        _ => return false,
                    },
                    b"propstat" => {
                        let Some(current_propstat) = propstat.take() else {
                            return false;
                        };
                        if current_propstat.has_collection
                            && current_propstat.status_success == Some(true)
                        {
                            let Some(current_response) = response.as_mut() else {
                                return false;
                            };
                            current_response.collection = true;
                        }
                    }
                    b"response" => {
                        let Some(current) = response.take() else {
                            return false;
                        };
                        let Some(href) = current.href else {
                            return false;
                        };
                        if collection_href_matches(root, path, &href) {
                            if matched || !current.collection {
                                return false;
                            }
                            matched = true;
                        }
                    }
                    b"multistatus" => {
                        if !stack.is_empty() || root_closed {
                            return false;
                        }
                        root_closed = true;
                        phase = DocumentPhase::After;
                    }
                    _ => {}
                }
            }
            Ok((_, Event::Comment(_) | Event::PI(_))) => {
                if phase == DocumentPhase::Before {
                    prolog_consumed = true;
                }
            }
            Ok((_, Event::Decl(declaration))) => {
                if phase != DocumentPhase::Before || declaration_seen || prolog_consumed {
                    return false;
                }
                if !validate_xml_declaration(&declaration) {
                    return false;
                }
                declaration_seen = true;
            }
            Ok((_, Event::DocType(_))) => return false,
            Ok((_, Event::Eof)) => {
                return phase == DocumentPhase::After
                    && root_closed
                    && stack.is_empty()
                    && capture.is_none()
                    && matched
            }
            Err(_) => return false,
        }
    }
}

// Frozen desktop listing document machine. Property subtrees are opaque;
// response and propstat controls retain their distinct placement/status rules.
fn parse_listing_hrefs(xml: &[u8]) -> Option<Vec<String>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut hrefs = Vec::new();
    let mut depth = 0_usize;
    let mut response_href = None;
    let mut response_status_seen = false;
    let mut propstat_seen = false;
    let mut in_propstat = false;
    let mut propstat_status_seen = false;
    let mut prop_depth = None;
    let mut in_href = false;
    let mut in_status = false;
    let mut text = String::new();
    #[derive(Eq, PartialEq)]
    enum DocumentPhase {
        Before,
        Inside,
        After,
    }
    let mut phase = DocumentPhase::Before;
    let mut declaration_seen = false;
    let mut prolog_consumed = false;
    macro_rules! start_element {
        ($is_dav:expr, $local:expr) => {{
            if phase == DocumentPhase::After {
                return None;
            }
            if depth == 0 {
                if phase != DocumentPhase::Before || !$is_dav || $local != b"multistatus" {
                    return None;
                }
                phase = DocumentPhase::Inside;
            } else if depth == 1 {
                if !$is_dav || $local != b"response" {
                    return None;
                }
                response_href = None;
                response_status_seen = false;
                propstat_seen = false;
                in_propstat = false;
                propstat_status_seen = false;
                prop_depth = None;
            } else if prop_depth.is_some() {
                // A DAV:prop value is opaque provider data.  In
                // particular, nested extension elements (or DAV names
                // such as href/status) are not response control fields.
            } else if $is_dav && $local == b"prop" && depth == 3 && in_propstat {
                prop_depth = Some(depth + 1);
            } else if $local == b"propstat" {
                if !$is_dav || depth != 2 || in_propstat || response_status_seen {
                    return None;
                }
                propstat_seen = true;
                in_propstat = true;
                propstat_status_seen = false;
            } else if $local == b"href" {
                if !$is_dav || depth != 2 || in_href {
                    return None;
                }
                in_href = true;
                text.clear();
            } else if $local == b"status" {
                if !$is_dav || in_status {
                    return None;
                }
                if depth == 2 {
                    if response_status_seen || propstat_seen {
                        return None;
                    }
                    response_status_seen = true;
                } else if depth == 3 && in_propstat {
                    if propstat_status_seen {
                        return None;
                    }
                    propstat_status_seen = true;
                } else {
                    return None;
                }
                in_status = true;
                text.clear();
            }
            depth += 1;
        }};
    }
    macro_rules! end_element {
        ($is_dav:expr, $local:expr) => {{
            if depth == 0 {
                return None;
            }
            if let Some(open_depth) = prop_depth {
                if depth == open_depth {
                    if !$is_dav || $local != b"prop" {
                        return None;
                    }
                    prop_depth = None;
                }
            } else if $local == b"href" {
                if !$is_dav || !in_href || depth != 3 || text.is_empty() {
                    return None;
                }
                response_href = Some(text.clone());
                in_href = false;
            } else if $local == b"status" {
                if !$is_dav || !in_status || !is_success_http_status_line(&text) {
                    return None;
                }
                in_status = false;
            } else if $local == b"propstat" {
                if !$is_dav
                    || depth != 3
                    || !in_propstat
                    || !propstat_status_seen
                    || in_status
                    || prop_depth.is_some()
                {
                    return None;
                }
                in_propstat = false;
            } else if $local == b"response" {
                if depth != 2
                    || !$is_dav
                    || response_href.is_none()
                    || in_propstat
                    || in_status
                    || prop_depth.is_some()
                {
                    return None;
                }
                hrefs.push(response_href.take().unwrap());
            } else if $local == b"multistatus" {
                if !$is_dav || depth != 1 {
                    return None;
                }
                phase = DocumentPhase::After;
            }
            depth -= 1;
        }};
    }
    loop {
        match reader.read_resolved_event() {
            Ok((namespace, Event::Start(e))) => {
                let is_dav =
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == b"DAV:");
                let local = e.local_name().as_ref().to_vec();
                start_element!(is_dav, local);
            }
            Ok((namespace, Event::Empty(e))) => {
                let is_dav =
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == b"DAV:");
                let local = e.local_name().as_ref().to_vec();
                start_element!(is_dav, local);
                end_element!(is_dav, local);
            }
            Ok((_, Event::Text(e))) => {
                if in_href || in_status {
                    let value = e.xml10_content().ok()?;
                    text.push_str(&unescape(&value).ok()?);
                } else if prop_depth.is_some_and(|open_depth| depth > open_depth) {
                    let value = e.xml10_content().ok()?;
                    unescape(&value).ok()?;
                } else {
                    let raw: &[u8] = e.as_ref();
                    if !raw.iter().all(|byte| is_xml_s(*byte)) {
                        return None;
                    }
                    if phase == DocumentPhase::Before {
                        prolog_consumed = true;
                    }
                }
            }
            Ok((_, Event::GeneralRef(e))) => {
                let name = std::str::from_utf8(e.as_ref()).ok()?;
                let escaped = format!("&{name};");
                let value = unescape(&escaped).ok()?;
                if in_href || in_status {
                    text.push_str(&value);
                } else if !prop_depth.is_some_and(|open_depth| depth > open_depth) {
                    return None;
                }
            }
            Ok((_, Event::CData(e))) => {
                if in_href || in_status {
                    let raw: &[u8] = e.as_ref();
                    let Ok(value) = std::str::from_utf8(raw) else {
                        return None;
                    };
                    text.push_str(value);
                } else if prop_depth.is_some_and(|open_depth| depth > open_depth) {
                    if std::str::from_utf8(e.as_ref()).is_err() {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            Ok((namespace, Event::End(e))) => {
                let is_dav =
                    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == b"DAV:");
                let local = e.local_name().as_ref().to_vec();
                end_element!(is_dav, local);
            }
            Ok((_, Event::Comment(_) | Event::PI(_))) => {
                if phase == DocumentPhase::Before {
                    prolog_consumed = true;
                }
            }
            Ok((_, Event::Decl(declaration))) => {
                if phase != DocumentPhase::Before || declaration_seen || prolog_consumed {
                    return None;
                }
                if !validate_xml_declaration(&declaration) {
                    return None;
                }
                declaration_seen = true;
            }
            Ok((_, Event::DocType(_))) => return None,
            Ok((_, Event::Eof))
                if phase == DocumentPhase::After
                    && depth == 0
                    && !in_href
                    && !in_status
                    && prop_depth.is_none() =>
            {
                break
            }
            Ok((_, Event::Eof)) => return None,
            Err(_) => return None,
        }
    }
    Some(hrefs)
}

fn parse_listing(root: &WebDavRootV1, directory: &str, xml: &[u8]) -> Option<Vec<String>> {
    let directory_url = Url::parse(&root.canonical_url)
        .ok()?
        .join(&format!("{directory}/"))
        .ok()?;
    let mut entries = Vec::new();
    for href in parse_listing_hrefs(xml)? {
        match directory_child(root, directory, &directory_url, &href)? {
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

    fn dav_document(inner: &str) -> Vec<u8> {
        format!("<d:multistatus xmlns:d='DAV:'>{inner}</d:multistatus>").into_bytes()
    }

    #[test]
    fn empty_controls_follow_expanded_listing_semantics() {
        let root = webdav_root_v1("https://example.test/dav", "alice").unwrap();
        // All frozen control elements, in their relevant structural context.
        for (name, before, after, accepted) in [
            ("multistatus", "", "", true),
            ("response", "<d:multistatus xmlns:d='DAV:'>", "</d:multistatus>", false),
            ("href", "<d:multistatus xmlns:d='DAV:'><d:response>", "</d:response></d:multistatus>", false),
            ("status", "<d:multistatus xmlns:d='DAV:'><d:response><d:href>/dav/activations/</d:href>", "</d:response></d:multistatus>", false),
            ("propstat", "<d:multistatus xmlns:d='DAV:'><d:response><d:href>/dav/activations/</d:href>", "</d:response></d:multistatus>", false),
            ("prop", "<d:multistatus xmlns:d='DAV:'><d:response><d:href>/dav/activations/</d:href><d:propstat>", "<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>", true),
            ("resourcetype", "<d:multistatus xmlns:d='DAV:'><d:response><d:href>/dav/activations/</d:href><d:propstat><d:prop>", "</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>", true),
            ("collection", "<d:multistatus xmlns:d='DAV:'><d:response><d:href>/dav/activations/</d:href><d:propstat><d:prop><d:resourcetype>", "</d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>", true),
        ] {
            let empty = format!("{before}<d:{name} xmlns:d='DAV:' />{after}");
            let expanded = format!("{before}<d:{name} xmlns:d='DAV:'></d:{name}>{after}");
            let result = parse_listing(&root, "activations", empty.as_bytes());
            assert_eq!(result, parse_listing(&root, "activations", expanded.as_bytes()), "{name}");
            assert_eq!(result.is_some(), accepted, "{name}");
            // The two proof spellings yield the same result in these contexts.
            assert_eq!(depth_zero_is_collection(&root, "activations", empty.as_bytes()), depth_zero_is_collection(&root, "activations", expanded.as_bytes()), "{name}");
        }
        for control in ["<d:propstat/>", "<d:status/>"] {
            let body = dav_document(&format!(
                "<d:response><d:href>/dav/activations/a.json</d:href>{control}</d:response>"
            ));
            assert_eq!(
                block(adapter(vec![response(207, &body)]).list_directory("activations")),
                DirectoryListResultV1::Indeterminate
            );
        }
    }

    #[test]
    fn opaque_listing_properties_do_not_capture_href_status_or_cdata() {
        for property in [
            "<d:owner><d:href>owner</d:href></d:owner>",
            "<d:displayname><![CDATA[activations]]></d:displayname>",
            "<x:property xmlns:x='urn:custom'><d:status>not an HTTP status</d:status><x:nested>text</x:nested></x:property>",
            "<d:displayname>a&amp;b&#32;&#x41;</d:displayname>",
            "<x:property xmlns:x='urn:custom'><d:response><d:propstat><d:href>opaque</d:href></d:propstat></d:response></x:property>",
        ] {
            let body = dav_document(&format!("<d:response><d:href>/dav/activations/a.json</d:href><d:propstat><d:prop>{property}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"));
            assert_eq!(block(adapter(vec![response(207, &body)]).list_directory("activations")), DirectoryListResultV1::Entries(vec!["activations/a.json".into()]), "{property}");
        }
    }

    #[test]
    fn collection_proof_and_listing_have_distinct_failed_property_policies() {
        let good = "<d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>";
        let unrelated = "<d:propstat><d:prop><d:unsupported/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>";
        for properties in [format!("{good}{unrelated}"), format!("{unrelated}{good}")] {
            let body = dav_document(&format!(
                "<d:response><d:href>/dav/activations</d:href>{properties}</d:response>"
            ));
            assert!(block(
                adapter(vec![response(405, b""), response(207, &body)])
                    .ensure_collection("activations")
            )
            .is_ok());
            assert_eq!(
                block(adapter(vec![response(207, &body)]).list_directory("activations")),
                DirectoryListResultV1::Indeterminate
            );
        }
    }

    #[test]
    fn frozen_declarations_and_document_phases_are_preserved() {
        let root = webdav_root_v1("https://example.test/dav", "alice").unwrap();
        let empty = "<d:multistatus xmlns:d='DAV:'/>";
        for declaration in [
            "",
            "<?xml version='1.0'?>",
            "<?xml version = '1.0' encoding = 'utf-8' standalone = 'yes'?>",
            "<?xml version='1.0' standalone='no'?>",
            "\u{feff}<?xml version='1.0'?>",
        ] {
            assert_eq!(
                parse_listing(
                    &root,
                    "activations",
                    format!("{declaration}{empty}").as_bytes()
                ),
                Some(vec![])
            );
        }
        for declaration in [
            "<?xml version='2.0'?>",
            "<?xml encoding='utf-8'?>",
            "<?xml version='1.0' version='1.0'?>",
            "<?xml version='1.0' encoding='windows-1252'?>",
            "<?xml version='1.0' vendor='x'?>",
            "<?xml version='1.0' standalone='invalid'?>",
            "<?xml version='1.0' malformed?>",
            "<?xml version='1.0'?><?xml version='1.0'?>",
            " <?xml version='1.0'?>",
            "<!--before--><?xml version='1.0'?>",
            "<?xml version\x0b='1.0'?>",
        ] {
            assert_eq!(
                parse_listing(
                    &root,
                    "activations",
                    format!("{declaration}{empty}").as_bytes()
                ),
                None,
                "{declaration}"
            );
        }
        for body in [
            format!("{empty}<?xml version='1.0'?>"),
            "<d:multistatus xmlns:d='DAV:'><?xml version='1.0'?></d:multistatus>".to_owned(),
            format!("{empty}{empty}"),
            format!("{empty}junk"),
            "<html/>".into(),
            "<d:multistatus xmlns:d='DAV:'>".into(),
            "".into(),
        ] {
            assert_eq!(
                parse_listing(&root, "activations", body.as_bytes()),
                None,
                "{body}"
            );
        }
    }

    #[test]
    fn listing_control_placement_and_status_width_match_frozen_policy() {
        let root = webdav_root_v1("https://example.test/dav", "alice").unwrap();
        for inner in [
            "<d:response><d:href>/dav/activations/a.json</d:href><d:status>HTTP/1.1 0200 OK</d:status></d:response>",
            "<d:response><d:href>/dav/activations/a.json</d:href><d:status>HTTP/1.1 200 OK</d:status><d:propstat><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
            "<d:response><d:href>/dav/activations/a.json</d:href><d:propstat><d:status>HTTP/1.1 200 OK</d:status><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
            "<d:response><d:href>/dav/activations/a.json</d:href><d:propstat><d:prop><d:displayname>text</d:displayname></d:prop></d:propstat></d:response>",
            "<d:response><d:response><d:href>/dav/activations/a.json</d:href></d:response></d:response>",
        ] {
            assert_eq!(parse_listing(&root,"activations",&dav_document(inner)),None,"{inner}");
        }
    }

    #[test]
    fn collection_evidence_is_exact_successful_and_unique() {
        let root = webdav_root_v1("https://example.test/dav", "alice").unwrap();
        let good = "<d:response><d:href>/dav/activations</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>";
        assert!(depth_zero_is_collection(
            &root,
            "activations",
            &dav_document(good)
        ));
        assert!(!depth_zero_is_collection(
            &root,
            "activations",
            &dav_document(&format!("{good}{good}"))
        ));
        assert!(!depth_zero_is_collection(
            &root,
            "activations",
            &dav_document(&good.replace("/dav/activations", "/dav/other"))
        ));
        assert!(!depth_zero_is_collection(
            &root,
            "activations",
            &dav_document(&good.replace("200 OK", "404 Not Found"))
        ));
        assert!(!depth_zero_is_collection(
            &root,
            "activations",
            &dav_document(&good.replace(
                "<d:propstat>",
                "<d:status>HTTP/1.1 403 Forbidden</d:status><d:propstat>"
            ))
        ));
    }
    #[test]
    fn collection_proof_resolves_relative_href_from_root_but_listing_stays_directory_relative() {
        let root = webdav_root_v1("https://example.test/dav/", "alice").unwrap();
        let collection = "writers/123e4567-e89b-42d3-a456-426614174000/segments";
        let relative = format!("<d:response><d:href>{collection}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>");
        assert!(depth_zero_is_collection(
            &root,
            collection,
            &dav_document(&relative)
        ));
        for escaped in [
            "writers/123e4567-e89b-42d3-a456-426614174000/other",
            "writers/123e4567-e89b-42d3-a456-426614174000/../segments",
        ] {
            let body = relative.replace(collection, escaped);
            assert!(!depth_zero_is_collection(
                &root,
                collection,
                &dav_document(&body)
            ));
        }
        assert_eq!(
            parse_listing(
                &root,
                "activations",
                &dav_document("<d:response><d:href>a.json</d:href></d:response>")
            ),
            Some(vec!["activations/a.json".into()])
        );
    }
    #[test]
    fn xml10_status_text_does_not_normalize_nel_and_entities_still_unescape() {
        let root = webdav_root_v1("https://example.test/dav", "alice").unwrap();
        let nel_listing = dav_document("<d:response><d:href>/dav/activations/a.json</d:href><d:status>HTTP/1.1\u{85}200 OK</d:status></d:response>");
        assert_eq!(parse_listing(&root, "activations", &nel_listing), None);
        let nel_collection = dav_document("<d:response><d:href>/dav/activations</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1\u{85}200 OK</d:status></d:propstat></d:response>");
        assert!(!depth_zero_is_collection(
            &root,
            "activations",
            &nel_collection
        ));
        let entity_status = dav_document("<d:response><d:href>/dav/activations/a.json</d:href><d:status>HTTP/1.1&#32;200 OK</d:status></d:response>");
        assert_eq!(
            parse_listing(&root, "activations", &entity_status),
            Some(vec!["activations/a.json".into()])
        );
    }
    #[test]
    fn adapter_is_not_registered_with_s1_sync() {
        assert!(!include_str!("../commands.rs").contains("s2_lite::webdav_adapter"));
    }
}
