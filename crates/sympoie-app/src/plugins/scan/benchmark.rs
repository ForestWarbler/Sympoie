use super::*;

struct BenchmarkDirectory(PathBuf);

impl BenchmarkDirectory {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "sympoie-scan-benchmark-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir(&directory).expect("create isolated benchmark directory");
        Self(directory)
    }
}

impl Drop for BenchmarkDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct BenchmarkRun {
    result: ScanResult,
    elapsed: Duration,
    total: usize,
    cached: usize,
    probed: usize,
}

fn measure_scan(label: &str, config: ScanConfig) -> BenchmarkRun {
    let started = std::time::Instant::now();
    let task = ScanTask::start_config(config);
    let mut first_catalog = None;
    loop {
        if let Some(update) = task.poll() {
            let update = update.unwrap_or_else(|error| panic!("{label} scan failed: {error}"));
            if first_catalog.is_none() && !update.result.plugins.is_empty() {
                first_catalog = Some(started.elapsed());
            }
            if update.finished {
                let progress = task.progress();
                let elapsed = started.elapsed();
                assert_eq!(
                    progress.completed, progress.total,
                    "scan completion accounting"
                );
                eprintln!(
                    "SCAN_BENCH {label}: elapsed_ms={} first_catalog_ms={} bundles={} probes={} cache_hits={} plugins={} failures={}",
                    elapsed.as_millis(),
                    first_catalog.map_or_else(
                        || "none".to_owned(),
                        |elapsed| elapsed.as_millis().to_string()
                    ),
                    progress.total,
                    progress.probed,
                    progress.cached,
                    update.result.plugins.len(),
                    update.result.failures.len(),
                );
                return BenchmarkRun {
                    result: update.result,
                    elapsed,
                    total: progress.total,
                    cached: progress.cached,
                    probed: progress.probed,
                };
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(600),
            "{label} scan exceeded ten minutes"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "loads explicitly selected installed VST3 bundles in isolated probe processes"]
fn installed_library_scan_cold_and_warm() {
    let executable = PathBuf::from(
        std::env::var_os("SYMPOIE_SCAN_BENCH_EXE")
            .expect("set SYMPOIE_SCAN_BENCH_EXE to the rebuilt GUI executable"),
    );
    assert!(
        executable.is_file(),
        "benchmark executable does not exist: {}",
        executable.display()
    );
    let paths_value = std::env::var_os("SYMPOIE_SCAN_BENCH_PATHS")
        .expect("set SYMPOIE_SCAN_BENCH_PATHS to explicitly selected plugin folders");
    let paths: Vec<_> = std::env::split_paths(&paths_value).collect();
    assert!(
        !paths.is_empty() && paths.iter().all(|path| !path.as_os_str().is_empty()),
        "select at least one nonempty plugin folder"
    );
    let workers = std::env::var("SYMPOIE_SCAN_BENCH_WORKERS")
        .map(|value| {
            value
                .parse::<usize>()
                .expect("worker count must be an integer")
        })
        .unwrap_or(4);
    assert!(
        (1..=4).contains(&workers),
        "worker count must be between 1 and 4"
    );
    let directory = BenchmarkDirectory::new();
    let cache_path = directory.0.join("plugins.json");
    let config = || ScanConfig {
        executable: executable.clone(),
        paths: paths.clone(),
        extra_paths: Vec::new(),
        cache_path: Some(cache_path.clone()),
        mode: ScanMode::Cached,
        workers,
        timeout: Duration::from_secs(15),
    };
    eprintln!(
        "SCAN_BENCH executable={} workers={workers} folders={paths:?}",
        executable.display()
    );
    let cold = measure_scan("cold", config());
    assert!(cold.total > 0, "selected folders contain no VST3 bundles");
    assert_eq!(
        cold.cached, 0,
        "isolated cold run must not reuse the user's cache"
    );
    assert_eq!(
        cold.probed, cold.total,
        "cold run must probe every discovered bundle"
    );
    assert!(cache_path.is_file(), "cold run did not persist its cache");
    let warm = measure_scan("warm", config());
    assert_eq!(
        warm.total, cold.total,
        "installed library changed during the benchmark"
    );
    assert_eq!(
        warm.probed, 0,
        "unchanged warm scan must not instantiate plugins"
    );
    assert_eq!(
        warm.cached, warm.total,
        "warm scan must reuse all successful and failed probes"
    );
    let catalog = |result: &ScanResult| {
        let mut entries: Vec<_> = result
            .plugins
            .iter()
            .map(|plugin| {
                (
                    plugin.path.clone(),
                    plugin.class_id.clone(),
                    plugin.name.clone(),
                    plugin.vendor.clone(),
                    plugin.kind == sympoie::plugin::PluginKind::Instrument,
                )
            })
            .collect();
        entries.sort();
        entries
    };
    assert_eq!(
        catalog(&warm.result),
        catalog(&cold.result),
        "cache changed the plugin catalog"
    );
    let mut cold_failures = cold.result.failures;
    let mut warm_failures = warm.result.failures;
    cold_failures.sort();
    warm_failures.sort();
    assert_eq!(
        warm_failures, cold_failures,
        "cached probe failures must remain visible"
    );
    eprintln!(
        "SCAN_BENCH speedup={:.2}x",
        cold.elapsed.as_secs_f64() / warm.elapsed.as_secs_f64().max(0.000_001)
    );
}
