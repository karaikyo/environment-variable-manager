//! Explicit, session-only release discovery, independent of environment storage.
mod windows;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
pub use windows::WindowsTransport;
pub const MAX_RESPONSE_BYTES: usize = 1_048_576;
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RELEASES_URL: &str = "https://api.github.com/repos/karaikyo/environment-variable-manager/releases?per_page=100&page=1";
#[derive(Clone)]
pub struct CheckControl {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}
impl CheckControl {
    pub fn new(timeout: Duration) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now() + timeout,
        }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() {
            Err("Update check cancelled".into())
        } else if self.remaining().is_zero() {
            Err("Update check timed out; try again".into())
        } else {
            Ok(())
        }
    }
}
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub link: Option<String>,
}
/// Implementations must bound all operations by control's deadline and cancellation.
pub trait ReleaseTransport: Send + Sync {
    fn fetch(&self, url: &str, control: &CheckControl) -> Result<HttpResponse, String>;
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    version: String,
    url: String,
}
impl Release {
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn url(&self) -> &str {
        &self.url
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Failed(String),
}
pub struct UpdateChecker {
    transport: Arc<dyn ReleaseTransport>,
    status: UpdateStatus,
    worker: Option<std::thread::JoinHandle<Result<UpdateStatus, String>>>,
    control: Option<CheckControl>,
}
impl UpdateChecker {
    pub fn new(transport: Arc<dyn ReleaseTransport>) -> Self {
        Self {
            transport,
            status: UpdateStatus::Idle,
            worker: None,
            control: None,
        }
    }
    pub fn start(&mut self) -> bool {
        if self.worker.is_some() {
            return false;
        }
        let control = CheckControl::new(CHECK_TIMEOUT);
        let worker_control = control.clone();
        let transport = self.transport.clone();
        match std::thread::Builder::new()
            .name("release-check".into())
            .spawn(move || check_releases(transport.as_ref(), &worker_control))
        {
            Ok(worker) => {
                self.worker = Some(worker);
                self.control = Some(control);
                self.status = UpdateStatus::Checking;
                true
            }
            Err(_) => {
                self.status = UpdateStatus::Failed("Cannot start update check; try again".into());
                false
            }
        }
    }
    /// Poll on the UI thread. A completed response never calls into a window.
    pub fn poll(&mut self) -> bool {
        if !self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            return false;
        }
        let result = self.worker.take().expect("finished worker").join();
        let control = self.control.take().expect("worker control");
        self.status = match result {
            // The worker checks its deadline before returning. A delayed UI poll
            // does not turn a successfully completed request into a timeout.
            Ok(Ok(_)) if control.is_cancelled() => {
                UpdateStatus::Failed("Update check cancelled".into())
            }
            Ok(Ok(status)) => status,
            Ok(Err(error)) => UpdateStatus::Failed(error),
            Err(_) => UpdateStatus::Failed("Update check failed; try again".into()),
        };
        true
    }
    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }
    pub fn is_running(&self) -> bool {
        self.worker.is_some()
    }
    pub fn cancel(&self) {
        if let Some(control) = &self.control {
            control.cancel();
        }
    }
}
impl Drop for UpdateChecker {
    fn drop(&mut self) {
        self.cancel();
    }
}

const API_BASE: &str =
    "https://api.github.com/repos/karaikyo/environment-variable-manager/releases";
const WEB_BASE: &str = "https://github.com/karaikyo/environment-variable-manager/releases/tag/";
const MAX_PAGES: usize = 5;
const MAX_TOTAL_BYTES: usize = 2 * MAX_RESPONSE_BYTES;
fn version(text: &str) -> Option<[u32; 3]> {
    let parts: Vec<_> = text.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut numbers = [0; 3];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() || part.len() > 10 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        numbers[i] = part.parse().ok()?;
    }
    Some(numbers)
}
fn api_page(url: &str) -> Option<u32> {
    // GitHub's Link header uses the repository's numeric identity, even when
    // the original request used owner/name. Accept only this fixed repository.
    let query = url
        .strip_prefix(API_BASE)
        .or_else(|| url.strip_prefix("https://api.github.com/repositories/1449318990/releases"))?
        .strip_prefix('?')?;
    let mut page = None;
    let mut size = false;
    for parameter in query.split('&') {
        match parameter.split_once('=')? {
            ("per_page", "100") if !size => size = true,
            ("page", value)
                if page.is_none()
                    && !value.is_empty()
                    && value.len() <= 10
                    && value.bytes().all(|b| b.is_ascii_digit()) =>
            {
                page = Some(value.parse::<u32>().ok()?);
            }
            _ => return None,
        }
    }
    if size {
        page.filter(|page| *page > 0)
    } else {
        None
    }
}
pub(crate) fn valid_api_url(url: &str) -> bool {
    api_page(url).is_some()
}
fn next_page(link: Option<&str>) -> Result<Option<String>, String> {
    let Some(link) = link else {
        return Ok(None);
    };
    if link.is_empty() || link.len() > 8192 {
        return Err("Invalid release pagination".into());
    }
    let mut next = None;
    for segment in link.split(',') {
        let (address, relation) = segment
            .trim()
            .split_once(';')
            .ok_or("Invalid release pagination")?;
        let url = address
            .trim()
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .ok_or("Invalid release pagination")?;
        let page = api_page(url).ok_or("Unsafe release pagination")?;
        match relation.trim() {
            "rel=\"next\"" if next.is_none() => {
                next = Some(format!("{API_BASE}?per_page=100&page={page}"))
            }
            "rel=\"prev\"" | "rel=\"first\"" | "rel=\"last\"" => {}
            _ => return Err("Invalid release pagination".into()),
        }
    }
    Ok(next)
}
fn malformed(_: rusqlite::Error) -> String {
    "Invalid release metadata".into()
}
fn unique_fields(db: &rusqlite::Connection, object: &str) -> Result<(), String> {
    let duplicate: bool = db
        .query_row(
            "SELECT EXISTS(SELECT key FROM json_each(?1) GROUP BY key HAVING count(*)>1)",
            [object],
            |row| row.get(0),
        )
        .map_err(malformed)?;
    if duplicate {
        Err("Ambiguous release metadata".into())
    } else {
        Ok(())
    }
}
fn field(
    db: &rusqlite::Connection,
    object: &str,
    key: &str,
    expected: &str,
) -> Result<String, String> {
    let result: (Option<String>, Option<String>) = db
        .query_row(
            "SELECT json_type(?1,?2),CAST(json_extract(?1,?2) AS TEXT)",
            rusqlite::params![object, format!("$.{key}")],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(malformed)?;
    if result.0.as_deref() != Some(expected) {
        return Err("Invalid release metadata".into());
    }
    result.1.ok_or("Invalid release metadata".into())
}
fn boolean(db: &rusqlite::Connection, object: &str, key: &str) -> Result<bool, String> {
    let kind: Option<String> = db
        .query_row(
            "SELECT json_type(?1,?2)",
            rusqlite::params![object, format!("$.{key}")],
            |row| row.get(0),
        )
        .map_err(malformed)?;
    match kind.as_deref() {
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        _ => Err("Invalid release metadata".into()),
    }
}
fn page_releases(body: &[u8], control: &CheckControl) -> Result<Vec<([u32; 3], Release)>, String> {
    let text = std::str::from_utf8(body).map_err(|_| "Invalid release metadata")?;
    let db =
        rusqlite::Connection::open_in_memory().map_err(|_| "Cannot inspect release metadata")?;
    db.execute_batch("PRAGMA cache_size=-256; PRAGMA temp_store=MEMORY;")
        .map_err(malformed)?;
    let valid: bool = db
        .query_row("SELECT json_valid(?1)", [text], |row| row.get(0))
        .map_err(malformed)?;
    if !valid {
        return Err("Invalid release metadata".into());
    }
    let kind: String = db
        .query_row("SELECT json_type(?1)", [text], |row| row.get(0))
        .map_err(malformed)?;
    if kind != "array" {
        return Err("Invalid release metadata".into());
    }
    let count: u32 = db
        .query_row("SELECT json_array_length(?1)", [text], |row| row.get(0))
        .map_err(malformed)?;
    if count > 100 {
        return Err("Release page exceeds limit".into());
    }
    let mut query = db
        .prepare("SELECT value,type FROM json_each(?1)")
        .map_err(malformed)?;
    let objects = query
        .query_map([text], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(malformed)?;
    let mut releases = Vec::new();
    for object in objects {
        control.check()?;
        let (object, kind) = object.map_err(malformed)?;
        if kind != "object" {
            return Err("Invalid release metadata".into());
        }
        unique_fields(&db, &object)?;
        if boolean(&db, &object, "draft")? {
            continue;
        }
        let tag = field(&db, &object, "tag_name", "text")?;
        let Some(numbers) = tag.strip_prefix('v').and_then(version) else {
            continue;
        };
        boolean(&db, &object, "prerelease")?;
        let published: Option<String> = db
            .query_row("SELECT json_type(?1,'$.published_at')", [&object], |row| {
                row.get(0)
            })
            .map_err(malformed)?;
        if published.as_deref() == Some("null") {
            continue;
        }
        if field(&db, &object, "published_at", "text")?
            .trim()
            .is_empty()
        {
            return Err("Invalid release metadata".into());
        }
        let assets = field(&db, &object, "assets", "array")?;
        let expected_name = format!("environment-variable-manager-{}-windows-x64.zip", &tag[1..]);
        let mut matching = 0;
        let mut asset_query = db
            .prepare("SELECT value,type FROM json_each(?1)")
            .map_err(malformed)?;
        for asset in asset_query
            .query_map([&assets], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(malformed)?
        {
            control.check()?;
            let (asset, kind) = asset.map_err(malformed)?;
            if kind != "object" {
                return Err("Invalid release asset".into());
            }
            unique_fields(&db, &asset)?;
            let name = field(&db, &asset, "name", "text")?;
            let state = field(&db, &asset, "state", "text")?;
            if name == expected_name && state == "uploaded" {
                matching += 1;
            }
        }
        if matching > 1 {
            return Err("Ambiguous release asset".into());
        }
        if matching == 0 {
            continue;
        }
        let url = format!("{WEB_BASE}{tag}");
        let url_kind: Option<String> = db
            .query_row("SELECT json_type(?1,'$.html_url')", [&object], |row| {
                row.get(0)
            })
            .map_err(malformed)?;
        if url_kind.is_some() && field(&db, &object, "html_url", "text")? != url {
            return Err("Unsafe release page address".into());
        }
        releases.push((
            numbers,
            Release {
                version: tag[1..].into(),
                url,
            },
        ));
    }
    Ok(releases)
}
fn check_releases(
    transport: &dyn ReleaseTransport,
    control: &CheckControl,
) -> Result<UpdateStatus, String> {
    let current = version(CURRENT_VERSION).ok_or("Unsupported application version")?;
    let mut url = RELEASES_URL.to_owned();
    let mut visited = Vec::new();
    let mut total = 0usize;
    let mut newest: Option<([u32; 3], Release)> = None;
    loop {
        control.check()?;
        if !valid_api_url(&url) || visited.contains(&url) {
            return Err("Unsafe or cyclic release pagination".into());
        }
        if visited.len() == MAX_PAGES {
            return Err("Release check incomplete: page limit reached".into());
        }
        visited.push(url.clone());
        let response = transport.fetch(&url, control)?;
        control.check()?;
        if response.status == 403 || response.status == 429 {
            return Err("GitHub request limited; try again later".into());
        }
        if response.status != 200 {
            return Err(format!(
                "GitHub check failed (HTTP {}); try again",
                response.status
            ));
        }
        total = total
            .checked_add(response.body.len())
            .ok_or("Release response exceeds limit")?;
        if response.body.len() > MAX_RESPONSE_BYTES || total > MAX_TOTAL_BYTES {
            return Err("Release response exceeds limit".into());
        }
        for release in page_releases(&response.body, control)? {
            if newest.as_ref().is_none_or(|current| release.0 > current.0) {
                newest = Some(release);
            }
        }
        match next_page(response.link.as_deref())? {
            Some(next) => {
                if api_page(&next) != Some(visited.len() as u32 + 1) {
                    return Err("Release check incomplete: invalid page sequence".into());
                }
                url = next;
            }
            None => break,
        }
    }
    control.check()?;
    let (numbers, release) = newest.ok_or("No usable published Windows x64 release found")?;
    Ok(if numbers > current {
        UpdateStatus::Available(release)
    } else {
        UpdateStatus::UpToDate
    })
}
