use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sympoie::plugin::{PluginDescriptor, PluginKind};

use super::probe::ProbeReport;

const CACHE_VERSION: u32 = 1;
const HOST_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "/vst3-host-0.10.0/probe-1");
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_WORKERS: usize = 4;
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_REPORT_BYTES: u64 = 8 * 1024 * 1024;
static SCAN_SERIAL: AtomicU64 = AtomicU64::new(0);
static CACHE_GENERATIONS: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanMode {
    Cached,
    Force,
}

#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    pub plugins: Vec<PluginDescriptor>,
    pub failures: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanProgress {
    pub completed: usize,
    pub total: usize,
    pub cached: usize,
    pub probed: usize,
}

#[derive(Clone, Debug)]
pub struct ScanUpdate {
    pub result: ScanResult,
    pub extra_paths: Vec<PathBuf>,
    pub finished: bool,
}

pub struct ScanTask {
    receiver: mpsc::Receiver<Result<ScanUpdate, String>>,
    progress: Arc<Mutex<ScanProgress>>,
    control: Arc<ScanControl>,
    delivered_terminal: AtomicBool,
}

#[derive(Default)]
struct ScanControl {
    cancelled: AtomicBool,
    children: Mutex<Vec<Weak<Mutex<Child>>>>,
    directory: Mutex<Option<PathBuf>>,
}

struct ScanConfig {
    executable: PathBuf,
    paths: Vec<PathBuf>,
    extra_paths: Vec<PathBuf>,
    cache_path: Option<PathBuf>,
    mode: ScanMode,
    workers: usize,
    timeout: Duration,
}

impl ScanTask {
    pub fn start(extra_paths: Vec<PathBuf>, mode: ScanMode) -> Self {
        let config = ScanConfig {
            executable: std::env::current_exe().unwrap_or_default(),
            paths: standard_paths(),
            extra_paths,
            cache_path: cache_path(),
            mode,
            workers: PROBE_WORKERS,
            timeout: PROBE_TIMEOUT,
        };
        Self::start_config(config)
    }

    fn start_config(config: ScanConfig) -> Self {
        let cache_access = CacheAccess::new(config.cache_path.clone());
        let (sender, receiver) = mpsc::channel();
        let progress = Arc::new(Mutex::new(ScanProgress::default()));
        let control = Arc::new(ScanControl::default());
        let worker_progress = Arc::clone(&progress);
        let worker_control = Arc::clone(&control);
        thread::spawn(move || {
            if let Err(error) = scan(
                config,
                &sender,
                &worker_progress,
                &worker_control,
                &cache_access,
            ) && !worker_control.cancelled.load(Ordering::Acquire)
            {
                let _ = sender.send(Err(error));
            }
        });
        Self {
            receiver,
            progress,
            control,
            delivered_terminal: AtomicBool::new(false),
        }
    }

    pub fn progress(&self) -> ScanProgress {
        self.progress.lock().map(|value| *value).unwrap_or_default()
    }

    pub fn poll(&self) -> Option<Result<ScanUpdate, String>> {
        if self.delivered_terminal.load(Ordering::Relaxed) {
            return None;
        }
        let mut latest = None;
        loop {
            match self.receiver.try_recv() {
                Ok(update) => latest = Some(update),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if latest.is_none() {
                        latest = Some(Err("The plugin scanner stopped unexpectedly".into()));
                    }
                    break;
                }
            }
        }
        if matches!(&latest, Some(Err(_))) || matches!(&latest, Some(Ok(update)) if update.finished)
        {
            self.delivered_terminal.store(true, Ordering::Relaxed);
        }
        latest
    }

    #[cfg(test)]
    pub fn from_updates(receiver: mpsc::Receiver<Result<ScanUpdate, String>>) -> Self {
        Self {
            receiver,
            progress: Arc::new(Mutex::new(ScanProgress::default())),
            control: Arc::new(ScanControl::default()),
            delivered_terminal: AtomicBool::new(false),
        }
    }
}

impl Drop for ScanTask {
    fn drop(&mut self) {
        self.control.cancelled.store(true, Ordering::Release);
        if let Ok(children) = self.control.children.lock() {
            for child in children.iter().filter_map(Weak::upgrade) {
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
            }
        }
        if let Ok(directory) = self.control.directory.lock()
            && let Some(directory) = &*directory
        {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct CacheEntry {
    path: PathBuf,
    fingerprint: u64,
    report: ProbeReport,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    platform: String,
    host_version: String,
    extra_paths: Vec<PathBuf>,
    entries: Vec<CacheEntry>,
}

impl Default for CacheFile {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            host_version: HOST_VERSION.into(),
            extra_paths: Vec::new(),
            entries: Vec::new(),
        }
    }
}

fn standard_paths() -> Vec<PathBuf> {
    let paths = vst3_host::discovery::scan_standard_paths();
    #[cfg(target_os = "windows")]
    {
        let mut paths = paths;
        if let Some(common) = std::env::var_os("CommonProgramFiles") {
            paths.push(PathBuf::from(common).join("VST3"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            paths.push(PathBuf::from(local).join("Programs/Common/VST3"));
        }
        paths
    }
    #[cfg(not(target_os = "windows"))]
    paths
}

fn cache_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SYMPOIE_VST3_CACHE") {
        return Some(PathBuf::from(path));
    }
    #[cfg(target_os = "macos")]
    let root =
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches/Sympoie"));
    #[cfg(target_os = "windows")]
    let root =
        std::env::var_os("LOCALAPPDATA").map(|home| PathBuf::from(home).join("Sympoie/Cache"));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .map(|root| root.join("sympoie"));
    root.map(|root| root.join("vst3-cache-v1.json"))
}

fn read_cache(path: Option<&Path>) -> CacheFile {
    let read = || {
        let path = path?;
        if fs::metadata(path).ok()?.len() > 64 * 1024 * 1024 {
            return None;
        }
        let cache: CacheFile = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        let expected = CacheFile::default();
        if cache.version == expected.version
            && cache.platform == expected.platform
            && cache.host_version == expected.host_version
        {
            Some(cache)
        } else {
            Some(CacheFile {
                extra_paths: cache.extra_paths,
                ..expected
            })
        }
    };
    read().unwrap_or_default()
}

struct CacheAccess {
    path: Option<PathBuf>,
    generation: u64,
}

impl CacheAccess {
    fn new(path: Option<PathBuf>) -> Self {
        let generation = SCAN_SERIAL.fetch_add(1, Ordering::Relaxed);
        if let Some(path) = &path {
            CACHE_GENERATIONS
                .get_or_init(Mutex::default)
                .lock()
                .unwrap()
                .insert(path.clone(), generation);
        }
        Self { path, generation }
    }
}

fn write_cache(access: &CacheAccess, cache: &CacheFile) {
    let Some(path) = access.path.as_deref() else {
        return;
    };
    let generations = CACHE_GENERATIONS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap();
    if generations.get(path) != Some(&access.generation) {
        return;
    }
    let save = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!(
            "{}-{}.tmp",
            std::process::id(),
            SCAN_SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let bytes = serde_json::to_vec(cache)?;
        fs::write(&temporary, bytes)?;
        let result = fs::rename(&temporary, path);
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    };
    let _ = save();
}

fn normalized_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut paths: Vec<_> = paths
        .into_iter()
        .map(|path| path.canonicalize().unwrap_or(path))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

fn discover(paths: Vec<PathBuf>, cancelled: &AtomicBool) -> (Vec<PathBuf>, Vec<String>) {
    let mut pending = paths;
    let mut visited = HashSet::new();
    let mut candidates = Vec::new();
    let mut failures = Vec::new();
    while let Some(path) = pending.pop() {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let Ok(path) = path.canonicalize() else {
            continue;
        };
        if !visited.insert(path.clone()) {
            continue;
        }
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("vst3"))
        {
            candidates.push(path);
            continue;
        }
        if path.is_dir() {
            match fs::read_dir(&path) {
                Ok(entries) => {
                    pending.extend(entries.filter_map(Result::ok).map(|entry| entry.path()))
                }
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
    }
    candidates.sort();
    (candidates, failures)
}

struct Fingerprint(u64);
impl Hasher for Fingerprint {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    }
}

fn fingerprint(path: &Path, cancelled: &AtomicBool) -> std::io::Result<u64> {
    let mut hash = Fingerprint(0xcbf29ce484222325);
    let mut pending = vec![path.to_owned()];
    let mut visited = HashSet::new();
    let mut count = 0;
    while let Some(entry) = pending.pop() {
        count += 1;
        if cancelled.load(Ordering::Acquire) {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        if count > 100_000 {
            return Err(std::io::Error::other(
                "plugin bundle contains too many files",
            ));
        }
        entry.strip_prefix(path).unwrap_or(&entry).hash(&mut hash);
        let link = fs::symlink_metadata(&entry)?;
        if link.file_type().is_symlink() {
            fs::read_link(&entry)?.hash(&mut hash);
        }
        let metadata = fs::metadata(&entry)?;
        metadata.is_dir().hash(&mut hash);
        metadata.len().hash(&mut hash);
        metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .hash(&mut hash);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            metadata.dev().hash(&mut hash);
            metadata.ino().hash(&mut hash);
            metadata.ctime().hash(&mut hash);
            metadata.ctime_nsec().hash(&mut hash);
        }
        if metadata.is_dir() && visited.insert(entry.canonicalize()?) {
            let mut children: Vec<_> = fs::read_dir(&entry)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<std::io::Result<_>>()?;
            children.sort();
            pending.extend(children);
        }
    }
    Ok(hash.finish())
}

struct ScanDirectory(PathBuf);
impl ScanDirectory {
    fn create() -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "sympoie-scan-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            SCAN_SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for ScanDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Probe {
    child: Arc<Mutex<Child>>,
    path: PathBuf,
    fingerprint: Option<u64>,
    output: PathBuf,
    started: Instant,
}

impl Probe {
    fn start(
        executable: &Path,
        path: PathBuf,
        fingerprint: Option<u64>,
        output: PathBuf,
        control: &ScanControl,
    ) -> Result<Self, String> {
        let mut children = control.children.lock().unwrap();
        if control.cancelled.load(Ordering::Acquire) {
            return Err("scan cancelled".into());
        }
        let child = Command::new(executable)
            .arg("--probe-vst3")
            .arg(&path)
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("could not start plugin probe: {error}"))?;
        let child = Arc::new(Mutex::new(child));
        children.retain(|child| child.strong_count() > 0);
        children.push(Arc::downgrade(&child));
        Ok(Self {
            child,
            path,
            fingerprint,
            output,
            started: Instant::now(),
        })
    }

    fn poll(&mut self, timeout: Duration) -> Option<Result<ProbeReport, String>> {
        match self.child.lock().unwrap().try_wait() {
            Ok(Some(status)) if status.success() => Some(read_report(&self.output)),
            Ok(Some(status)) => Some(Err(format!("probe failed ({status})"))),
            Ok(None) if self.started.elapsed() < timeout => None,
            Ok(None) => Some(Err(format!(
                "probe timed out after {} seconds",
                timeout.as_secs()
            ))),
            Err(error) => Some(Err(format!("could not monitor plugin probe: {error}"))),
        }
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(&self.output);
    }
}

fn read_report(output: &Path) -> Result<ProbeReport, String> {
    let metadata = fs::metadata(output).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_REPORT_BYTES {
        return Err("probe metadata exceeded its size limit".into());
    }
    let bytes = fs::read(output).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid probe metadata: {error}"))
}

fn append_report(result: &mut ScanResult, path: &Path, report: &ProbeReport) {
    result.failures.extend(
        report
            .failures
            .iter()
            .map(|error| format!("{}: {error}", path.display())),
    );
    result
        .plugins
        .extend(report.plugins.iter().map(|plugin| PluginDescriptor {
            path: path.to_owned(),
            class_id: plugin.class_id.clone(),
            name: plugin.name.clone(),
            vendor: plugin.vendor.clone(),
            kind: if plugin.instrument {
                PluginKind::Instrument
            } else {
                PluginKind::Effect
            },
        }));
}

fn publish(
    sender: &mpsc::Sender<Result<ScanUpdate, String>>,
    result: &ScanResult,
    paths: &[PathBuf],
    finished: bool,
) {
    let mut result = result.clone();
    result.plugins.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.path.cmp(&b.path))
            .then(a.class_id.cmp(&b.class_id))
    });
    result
        .plugins
        .dedup_by(|a, b| a.path == b.path && a.class_id == b.class_id);
    let _ = sender.send(Ok(ScanUpdate {
        result,
        extra_paths: paths.to_vec(),
        finished,
    }));
}

fn scan(
    config: ScanConfig,
    sender: &mpsc::Sender<Result<ScanUpdate, String>>,
    progress: &Mutex<ScanProgress>,
    control: &ScanControl,
    cache_access: &CacheAccess,
) -> Result<(), String> {
    let cancelled = &control.cancelled;
    let mut cache = read_cache(config.cache_path.as_deref());
    cache.extra_paths = normalized_paths(cache.extra_paths.into_iter().chain(config.extra_paths));
    let paths = normalized_paths(
        config
            .paths
            .into_iter()
            .chain(cache.extra_paths.iter().cloned()),
    );
    let (candidates, failures) = discover(paths, cancelled);
    if cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let present: HashSet<_> = candidates.iter().cloned().collect();
    cache.entries.retain(|entry| present.contains(&entry.path));
    let old: HashMap<_, _> = cache
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.clone()))
        .collect();
    let mut result = ScanResult {
        plugins: Vec::new(),
        failures,
    };
    let mut counts = ScanProgress {
        total: candidates.len(),
        ..ScanProgress::default()
    };
    *progress.lock().unwrap() = counts;
    let mut pending = VecDeque::new();
    for path in candidates {
        if cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        match fingerprint(&path, cancelled) {
            Ok(fingerprint) => {
                if config.mode == ScanMode::Cached
                    && let Some(entry) = old
                        .get(&path)
                        .filter(|entry| entry.fingerprint == fingerprint)
                {
                    append_report(&mut result, &path, &entry.report);
                    counts.cached += 1;
                    counts.completed += 1;
                } else {
                    cache.entries.retain(|entry| entry.path != path);
                    pending.push_back((path, Some(fingerprint)));
                }
            }
            Err(_) => {
                cache.entries.retain(|entry| entry.path != path);
                pending.push_back((path, None));
            }
        }
    }
    if cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    *progress.lock().unwrap() = counts;
    write_cache(cache_access, &cache);
    publish(sender, &result, &cache.extra_paths, pending.is_empty());
    if pending.is_empty() || cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let directory = {
        let mut slot = control.directory.lock().unwrap();
        if cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let directory = ScanDirectory::create().map_err(|error| error.to_string())?;
        *slot = Some(directory.0.clone());
        directory
    };
    let mut active = Vec::new();
    let mut serial = 0;
    let mut last_save = Instant::now();
    'scanning: while !pending.is_empty() || !active.is_empty() {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let mut changed = false;
        while active.len() < config.workers.clamp(1, PROBE_WORKERS) {
            let Some((path, fingerprint)) = pending.pop_front() else {
                break;
            };
            let output = directory.0.join(format!("{serial}.json"));
            serial += 1;
            match Probe::start(
                &config.executable,
                path.clone(),
                fingerprint,
                output,
                control,
            ) {
                Ok(probe) => active.push(probe),
                Err(error) => {
                    if cancelled.load(Ordering::Acquire) {
                        break 'scanning;
                    }
                    let report = ProbeReport {
                        plugins: Vec::new(),
                        failures: vec![error],
                    };
                    append_report(&mut result, &path, &report);

                    counts.probed += 1;
                    counts.completed += 1;
                    changed = true;
                }
            }
        }
        let mut index = 0;
        while index < active.len() {
            let Some(report) = active[index].poll(config.timeout) else {
                index += 1;
                continue;
            };
            if cancelled.load(Ordering::Acquire) {
                break 'scanning;
            }
            let probe = active.swap_remove(index);
            let report = report.unwrap_or_else(|error| ProbeReport {
                plugins: Vec::new(),
                failures: vec![error],
            });
            append_report(&mut result, &probe.path, &report);
            if let Some(fingerprint) = probe.fingerprint {
                cache.entries.push(CacheEntry {
                    path: probe.path.clone(),
                    fingerprint,
                    report,
                });
            }
            counts.completed += 1;
            counts.probed += 1;
            changed = true;
        }
        if changed {
            *progress.lock().unwrap() = counts;
            if !pending.is_empty() || !active.is_empty() {
                publish(sender, &result, &cache.extra_paths, false);
            }
            if last_save.elapsed() >= Duration::from_millis(250) {
                write_cache(cache_access, &cache);
                last_save = Instant::now();
            }
        }
        if !pending.is_empty() || !active.is_empty() {
            thread::sleep(POLL_INTERVAL);
        }
    }
    drop(active);
    write_cache(cache_access, &cache);
    if !cancelled.load(Ordering::Acquire) {
        publish(sender, &result, &cache.extra_paths, true);
    }
    Ok(())
}

#[cfg(test)]
mod benchmark;

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sympoie-scan-test-{}-{}",
                std::process::id(),
                SCAN_SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn bundle(&self, name: &str) -> PathBuf {
            let path = self.0.join("plugins").join(format!("{name}.vst3"));
            fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
            fs::write(path.join("Contents/MacOS/processor"), b"binary fixture").unwrap();
            path
        }
        #[cfg(unix)]
        fn script(&self, script: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = self.0.join("probe");
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }
        fn config(&self, executable: &Path, mode: ScanMode) -> ScanConfig {
            ScanConfig {
                executable: executable.to_owned(),
                paths: vec![self.0.join("plugins")],
                extra_paths: Vec::new(),
                cache_path: Some(self.0.join("cache.json")),
                mode,
                workers: 4,
                timeout: Duration::from_secs(3),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn finish(task: &ScanTask) -> ScanUpdate {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(update) = task.poll() {
                let update = update.unwrap();
                if update.finished {
                    return update;
                }
            }
            assert!(Instant::now() < deadline, "test scanner did not finish");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[cfg(unix)]
    const SCRIPT: &str = r#"#!/bin/sh
printf '%s\n' "$2" >> "$0.calls"
case "$2" in
  *Fail.vst3) exit 9;;
  *Malformed.vst3) printf 'not-json' > "$3"; exit 0;;
  *Hung.vst3) printf '%s\n%s\n' "$$" "$3" > "$0.hung"; exec sleep 10;;
esac
printf '%s' '{"plugins":[{"path":"untrusted.vst3","class_id":"fixture","name":"Fixture","vendor":"Test","instrument":true}],"failures":[]}' > "$3"
"#;

    #[cfg(unix)]
    #[test]
    fn cache_reuses_success_and_failure_then_invalidates_nested_changes_and_prunes() {
        let fixture = Fixture::new();
        let good = fixture.bundle("Good");
        let bad = fixture.bundle("Fail");
        let executable = fixture.script(SCRIPT);
        let cold = ScanTask::start_config(fixture.config(&executable, ScanMode::Cached));
        let result = finish(&cold);
        assert_eq!(result.result.plugins.len(), 1);
        assert_eq!(result.result.plugins[0].path, good.canonicalize().unwrap());
        assert_eq!(result.result.failures.len(), 1);
        assert_eq!(
            cold.progress(),
            ScanProgress {
                total: 2,
                completed: 2,
                cached: 0,
                probed: 2
            }
        );
        let warm = ScanTask::start_config(fixture.config(&executable, ScanMode::Cached));
        assert_eq!(finish(&warm).result.plugins.len(), 1);
        assert_eq!(warm.progress().cached, 2);
        assert_eq!(warm.progress().probed, 0);

        fs::write(
            good.join("Contents/MacOS/processor"),
            b"replacement binary fixture",
        )
        .unwrap();
        let changed = ScanTask::start_config(fixture.config(&executable, ScanMode::Cached));
        finish(&changed);
        assert_eq!(changed.progress().cached, 1);
        assert_eq!(changed.progress().probed, 1);
        fs::remove_dir_all(&bad).unwrap();
        fixture.bundle("New");
        let added = ScanTask::start_config(fixture.config(&executable, ScanMode::Cached));
        assert!(finish(&added).result.failures.is_empty());
        assert_eq!(added.progress().cached, 1);
        assert_eq!(added.progress().probed, 1);
        let saved = read_cache(Some(&fixture.0.join("cache.json")));
        assert_eq!(saved.entries.len(), 2);
        assert!(saved.entries.iter().all(|entry| entry.path != bad));
        let forced = ScanTask::start_config(fixture.config(&executable, ScanMode::Force));
        finish(&forced);
        assert_eq!(forced.progress().cached, 0);
        assert_eq!(forced.progress().probed, 2);
    }

    #[cfg(unix)]
    #[test]
    fn parallel_probes_publish_success_before_hung_vendor_and_contain_errors() {
        let fixture = Fixture::new();
        for name in ["Good", "Fail", "Hung", "Malformed"] {
            fixture.bundle(name);
        }
        let executable = fixture.script(SCRIPT);
        let task = ScanTask::start_config(fixture.config(&executable, ScanMode::Cached));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_partial = false;
        let final_update = loop {
            let update = task
                .receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("test scanner did not publish its terminal result")
                .unwrap();
            if !update.finished
                && !update.result.plugins.is_empty()
                && !update
                    .result
                    .failures
                    .iter()
                    .any(|failure| failure.contains("timed out"))
            {
                saw_partial = true;
            }
            if update.finished {
                break update;
            }
        };
        assert!(
            saw_partial,
            "a slow vendor must not withhold successful results"
        );
        assert_eq!(final_update.result.plugins.len(), 1);
        assert_eq!(final_update.result.failures.len(), 3);
        for expected in ["probe failed", "invalid probe metadata", "timed out"] {
            assert!(
                final_update
                    .result
                    .failures
                    .iter()
                    .any(|failure| failure.contains(expected))
            );
        }
        let output = fs::read_to_string(executable.with_extension("hung")).unwrap();
        let temporary = Path::new(output.lines().nth(1).unwrap()).parent().unwrap();
        while temporary.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!temporary.exists());
    }

    #[cfg(unix)]
    #[test]
    fn dropping_scan_kills_child_and_removes_temporary_reports() {
        let fixture = Fixture::new();
        fixture.bundle("Hung");
        let executable = fixture.script(SCRIPT);
        let mut config = fixture.config(&executable, ScanMode::Cached);
        config.timeout = Duration::from_secs(15);
        let task = ScanTask::start_config(config);
        let marker = executable.with_extension("hung");
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        let marker = fs::read_to_string(marker).unwrap();
        let pid = marker.lines().next().unwrap();
        let temporary = Path::new(marker.lines().nth(1).unwrap()).parent().unwrap();
        let control = Arc::clone(&task.control);
        let dropped = Instant::now();
        drop(task);
        assert!(dropped.elapsed() < Duration::from_millis(500));
        loop {
            let alive = Command::new("kill")
                .args(["-0", pid])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            if !alive && !temporary.exists() && Arc::strong_count(&control) == 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "cancelled probe was not cleaned up"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let cache = read_cache(Some(&fixture.0.join("cache.json")));
        assert!(
            cache.entries.is_empty(),
            "cancelled probes must not be cached as vendor failures"
        );
    }

    #[cfg(unix)]
    #[test]
    fn custom_paths_survive_restart_and_corrupt_or_unwritable_cache_is_nonfatal() {
        let fixture = Fixture::new();
        fixture.bundle("Good");
        let executable = fixture.script(SCRIPT);
        let mut config = fixture.config(&executable, ScanMode::Cached);
        config.extra_paths = std::mem::take(&mut config.paths);
        let first = ScanTask::start_config(config);
        assert_eq!(finish(&first).extra_paths.len(), 1);
        let mut config = fixture.config(&executable, ScanMode::Cached);
        config.paths.clear();
        let saved_paths = ScanTask::start_config(config);
        assert_eq!(finish(&saved_paths).result.plugins.len(), 1);
        assert_eq!(saved_paths.progress().cached, 1);
        fs::write(fixture.0.join("cache.json"), b"corrupt").unwrap();
        let corrupt = ScanTask::start_config(fixture.config(&executable, ScanMode::Cached));
        assert_eq!(finish(&corrupt).result.plugins.len(), 1);
        assert_eq!(corrupt.progress().probed, 1);
        let mut config = fixture.config(&executable, ScanMode::Cached);
        config.cache_path = Some(fixture.0.clone());
        let unwritable = ScanTask::start_config(config);
        assert_eq!(finish(&unwritable).result.plugins.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_failures_are_retried_without_requiring_full_rescan() {
        let fixture = Fixture::new();
        fixture.bundle("Good");
        let missing = fixture.0.join("missing-executable");
        let failed = ScanTask::start_config(fixture.config(&missing, ScanMode::Cached));
        assert_eq!(finish(&failed).result.failures.len(), 1);
        let executable = fixture.script(SCRIPT);
        let retry = ScanTask::start_config(fixture.config(&executable, ScanMode::Cached));
        assert_eq!(finish(&retry).result.plugins.len(), 1);
        assert_eq!(retry.progress().probed, 1);
    }

    #[test]
    fn cache_schema_platform_and_host_changes_invalidate_and_stale_writes_are_ignored() {
        let fixture = Fixture::new();
        let path = fixture.0.join("cache.json");
        let old_access = CacheAccess::new(Some(path.clone()));
        let new_access = CacheAccess::new(Some(path.clone()));
        let mut cache = CacheFile {
            extra_paths: vec![PathBuf::from("new")],
            ..CacheFile::default()
        };
        write_cache(&new_access, &cache);
        let stale = CacheFile {
            extra_paths: vec![PathBuf::from("old")],
            ..CacheFile::default()
        };
        write_cache(&old_access, &stale);
        assert_eq!(
            read_cache(Some(&path)).extra_paths,
            vec![PathBuf::from("new")]
        );
        for field in ["version", "platform", "host"] {
            match field {
                "version" => cache.version += 1,
                "platform" => cache.platform.push_str("-other"),
                _ => cache.host_version.push_str("-other"),
            }
            cache.entries.push(CacheEntry {
                path: PathBuf::from("device.vst3"),
                fingerprint: 1,
                report: ProbeReport::default(),
            });
            write_cache(&new_access, &cache);
            let invalidated = read_cache(Some(&path));
            assert_eq!(invalidated.extra_paths, vec![PathBuf::from("new")]);
            assert!(invalidated.entries.is_empty());
            cache = CacheFile {
                extra_paths: vec![PathBuf::from("new")],
                ..CacheFile::default()
            };
        }
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_fingerprint_does_not_hide_a_loadable_plugin() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let bundle = fixture.bundle("Good");
        symlink("missing-resource", bundle.join("optional-resource")).unwrap();
        let executable = fixture.script(SCRIPT);
        for _ in 0..2 {
            let task = ScanTask::start_config(fixture.config(&executable, ScanMode::Cached));
            assert_eq!(finish(&task).result.plugins.len(), 1);
            assert_eq!(task.progress().probed, 1);
            assert_eq!(task.progress().cached, 0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn discovery_deduplicates_aliases_and_fingerprints_symlink_loops() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let bundle = fixture.bundle("Good");
        let alias = fixture.0.join("alias");
        symlink(fixture.0.join("plugins"), &alias).unwrap();
        symlink("..", bundle.join("Contents/loop")).unwrap();
        let cancelled = AtomicBool::new(false);
        let (candidates, failures) = discover(
            vec![alias, fixture.0.join("plugins"), bundle.clone()],
            &cancelled,
        );
        assert_eq!(candidates, vec![bundle.canonicalize().unwrap()]);
        assert!(failures.is_empty());
        let original = fingerprint(&bundle, &cancelled).unwrap();
        fs::write(
            bundle.join("Contents/MacOS/processor"),
            b"changed nested binary",
        )
        .unwrap();
        assert_ne!(fingerprint(&bundle, &cancelled).unwrap(), original);
    }
}
