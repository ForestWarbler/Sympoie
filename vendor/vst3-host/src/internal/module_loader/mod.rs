//! Platform-specific VST3 module loading implementations

use crate::error::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use vst3::{
    ComPtr,
    Steinberg::{
        tresult, IPluginFactory, IPluginFactory3, IPluginFactory3Trait, Vst::IHostApplication,
    },
};

/// Trait representing a loaded VST3 module
pub trait VstModule: Send {
    /// Get the plugin factory from the module
    fn get_factory(&self) -> Result<*mut IPluginFactory>;

    /// Set the module-global factory context. Shared modules retain the latest context
    /// until unload, even when the instance which supplied it is removed first.
    fn set_factory_host_context(
        &self,
        factory: &ComPtr<IPluginFactory3>,
        context: Option<ComPtr<IHostApplication>>,
    ) -> tresult {
        unsafe {
            factory.setHostContext(
                context
                    .as_ref()
                    .map_or(std::ptr::null_mut(), |context| context.as_ptr().cast()),
            )
        }
    }

    /// Get the path to the module (diagnostics / config record)
    #[allow(dead_code)]
    fn path(&self) -> &Path;
}

/// Trait for platform-specific module loaders
pub trait ModuleLoader {
    /// Load a VST3 module from the given path
    fn load(path: &Path) -> Result<Box<dyn VstModule>>;
}

/// Detect if a plugin requires private namespace loading due to C symbol conflicts.
/// Only consulted by the macOS loader (private namespaces are a dyld feature).
#[cfg(target_os = "macos")]
fn requires_private_namespace(path: &Path) -> bool {
    if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
        let filename_lower = filename.to_lowercase();

        // Plugins with C symbol conflicts (not Objective-C)
        if filename_lower.contains("ozone") ||
           filename_lower.contains("rx") ||      // iZotope RX series
           filename_lower.contains("neutron") || // iZotope Neutron  
           filename_lower.contains("insight")
        // iZotope Insight
        {
            log::info!(
                "Detected plugin requiring private namespace for C symbols: {}",
                filename
            );
            return true;
        }
    }

    false
}

/// Load a VST3 module using the appropriate platform-specific loader
pub fn load_module(path: &Path) -> Result<Box<dyn VstModule>> {
    // A bundle and a direct path to its executable refer to the same module on macOS.
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    #[cfg(target_os = "macos")]
    let canonical = canonical
        .ancestors()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "vst3")
        })
        .unwrap_or(&canonical)
        .to_path_buf();
    load_cached_module(&canonical, load_platform_module)
}

struct SharedModule {
    path: PathBuf,
    module: Mutex<Option<Box<dyn VstModule>>>,
    // Dropped after the module: SDK factories may read this raw host pointer from
    // global state during bundleExit/DeinitModule.
    host_context: Mutex<Option<ComPtr<IHostApplication>>>,
}

fn modules() -> &'static Mutex<HashMap<PathBuf, Arc<SharedModule>>> {
    static MODULES: OnceLock<Mutex<HashMap<PathBuf, Arc<SharedModule>>>> = OnceLock::new();
    MODULES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn load_cached_module(
    path: &Path,
    load: impl FnOnce(&Path) -> Result<Box<dyn VstModule>>,
) -> Result<Box<dyn VstModule>> {
    // Serialize load and final unload of the same OS library. CFBundleLoadExecutable
    // is not a per-instance reference count; one instance's unload invalidates all
    // sibling instances unless the host shares this lease.
    let mut cache = modules()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if let Some(module) = cache.get(path) {
        return Ok(Box::new(ModuleLease(Some(module.clone()))));
    }
    let module = Arc::new(SharedModule {
        path: path.to_path_buf(),
        module: Mutex::new(Some(load(path)?)),
        host_context: Mutex::new(None),
    });
    cache.insert(path.to_path_buf(), module.clone());
    Ok(Box::new(ModuleLease(Some(module))))
}

struct ModuleLease(Option<Arc<SharedModule>>);

impl ModuleLease {
    fn shared(&self) -> &SharedModule {
        self.0.as_ref().expect("live module lease")
    }
}

impl VstModule for ModuleLease {
    fn get_factory(&self) -> Result<*mut IPluginFactory> {
        self.shared()
            .module
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .expect("live module lease")
            .get_factory()
    }

    fn set_factory_host_context(
        &self,
        factory: &ComPtr<IPluginFactory3>,
        context: Option<ComPtr<IHostApplication>>,
    ) -> tresult {
        let mut retained = self
            .shared()
            .host_context
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let result = unsafe {
            factory.setHostContext(
                context
                    .as_ref()
                    .map_or(std::ptr::null_mut(), |context| context.as_ptr().cast()),
            )
        };
        if result == vst3::Steinberg::kResultOk || result == vst3::Steinberg::kResultTrue {
            *retained = context;
        }
        result
    }

    fn path(&self) -> &Path {
        &self.shared().path
    }
}

impl Drop for ModuleLease {
    fn drop(&mut self) {
        let mut cache = modules()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(module) = self.0.take() else {
            return;
        };
        // One cache reference plus this final lease. Hold the cache lock until the
        // real module's destructor completes so a new load cannot race bundleExit.
        if Arc::strong_count(&module) == 2 {
            cache.remove(&module.path);
        }
        drop(module);
    }
}

fn load_platform_module(path: &Path) -> Result<Box<dyn VstModule>> {
    #[cfg(target_os = "macos")]
    {
        if requires_private_namespace(path) {
            // Use private namespace loading for both Objective-C and C symbol conflicts
            log::info!(
                "Using private namespace loader for symbol isolation: {}",
                path.display()
            );
            private_namespace::PrivateNamespaceModuleLoader::load(path)
        } else {
            // Use standard CFBundle loader for plugins without conflicts
            log::debug!("Using standard CFBundle loader: {}", path.display());
            macos::MacOSModuleLoader::load(path)
        }
    }

    #[cfg(target_os = "windows")]
    {
        windows::WindowsModuleLoader::load(path)
    }

    #[cfg(target_os = "linux")]
    {
        linux::LinuxModuleLoader::load(path)
    }

    #[cfg(target_os = "android")]
    {
        linux::LinuxModuleLoader::load(path)
    }
}

#[cfg(test)]
mod shared_module_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use vst3::{Class, ComWrapper, Steinberg::*};

    struct FakeModule {
        path: PathBuf,
        on_drop: Option<Box<dyn FnOnce() + Send>>,
    }

    impl VstModule for FakeModule {
        fn get_factory(&self) -> Result<*mut IPluginFactory> {
            Ok(std::ptr::null_mut())
        }
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for FakeModule {
        fn drop(&mut self) {
            if let Some(callback) = self.on_drop.take() {
                callback();
            }
        }
    }

    fn fake(path: &Path, callback: impl FnOnce() + Send + 'static) -> Result<Box<dyn VstModule>> {
        Ok(Box::new(FakeModule {
            path: path.to_path_buf(),
            on_drop: Some(Box::new(callback)),
        }))
    }

    #[test]
    fn repeated_module_path_shares_initialization_until_final_lease() {
        let path = Path::new("/module-cache-tests/shared.vst3");
        let drops = Arc::new(AtomicUsize::new(0));
        let dropped = drops.clone();
        let first = load_cached_module(path, |path| {
            fake(path, move || {
                dropped.fetch_add(1, Ordering::SeqCst);
            })
        })
        .unwrap();
        let second = load_cached_module(path, |_| panic!("must share the first load")).unwrap();
        drop(first);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(second);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(!modules().lock().unwrap().contains_key(path));
    }

    #[test]
    fn failed_load_is_not_cached_and_distinct_paths_are_independent() {
        let first_path = Path::new("/module-cache-tests/failure.vst3");
        assert!(
            load_cached_module(first_path, |_| Err(crate::Error::Other("fixture".into()))).is_err()
        );
        assert!(!modules().lock().unwrap().contains_key(first_path));
        let first = load_cached_module(first_path, |path| fake(path, || {})).unwrap();
        let second_path = Path::new("/module-cache-tests/other.vst3");
        let second = load_cached_module(second_path, |path| fake(path, || {})).unwrap();
        assert_ne!(first.path(), second.path());
        drop(first);
        assert!(modules().lock().unwrap().contains_key(second_path));
        drop(second);
    }

    #[test]
    fn concurrent_reload_cannot_initialize_before_final_unload_completes() {
        let path = Path::new("/module-cache-tests/concurrent.vst3");
        let (entered_send, entered_receive) = std::sync::mpsc::channel();
        let (finish_send, finish_receive) = std::sync::mpsc::channel();
        let finished = Arc::new(AtomicUsize::new(0));
        let completed = finished.clone();
        let first = load_cached_module(path, |path| {
            fake(path, move || {
                entered_send.send(()).unwrap();
                finish_receive.recv().unwrap();
                completed.store(1, Ordering::SeqCst);
            })
        })
        .unwrap();
        let dropping = std::thread::spawn(move || drop(first));
        entered_receive.recv().unwrap();
        assert!(
            modules().try_lock().is_err(),
            "final unload keeps the registry serialized"
        );
        let (attempt_send, attempt_receive) = std::sync::mpsc::channel();
        let loading = std::thread::spawn(move || {
            attempt_send.send(()).unwrap();
            load_cached_module(path, |path| {
                assert_eq!(finished.load(Ordering::SeqCst), 1);
                fake(path, || {})
            })
            .unwrap()
        });
        attempt_receive.recv().unwrap();
        finish_send.send(()).unwrap();
        dropping.join().unwrap();
        drop(loading.join().unwrap());
    }

    struct RecordingHost(Arc<Mutex<Vec<&'static str>>>);
    impl Class for RecordingHost {
        type Interfaces = (IHostApplication,);
    }
    impl vst3::Steinberg::Vst::IHostApplicationTrait for RecordingHost {
        unsafe fn getName(&self, _: *mut vst3::Steinberg::Vst::String128) -> tresult {
            kResultFalse
        }
        unsafe fn createInstance(
            &self,
            _: *mut TUID,
            _: *mut TUID,
            _: *mut *mut std::ffi::c_void,
        ) -> tresult {
            kNoInterface
        }
    }
    impl Drop for RecordingHost {
        fn drop(&mut self) {
            self.0.lock().unwrap().push("host");
        }
    }

    #[test]
    fn retained_factory_host_context_outlives_module_unload() {
        let path = Path::new("/module-cache-tests/context.vst3");
        let events = Arc::new(Mutex::new(Vec::new()));
        let module_events = events.clone();
        let lease = load_cached_module(path, |path| {
            fake(path, move || module_events.lock().unwrap().push("module"))
        })
        .unwrap();
        let context = ComWrapper::new(RecordingHost(events.clone()));
        *modules()
            .lock()
            .unwrap()
            .get(path)
            .unwrap()
            .host_context
            .lock()
            .unwrap() = context.to_com_ptr::<IHostApplication>();
        drop(context);
        assert!(events.lock().unwrap().is_empty());
        drop(lease);
        assert_eq!(*events.lock().unwrap(), vec!["module", "host"]);
    }
}

/// Pure, platform-independent binary-header parsing for architecture-mismatch diagnostics.
///
/// Lives here (always compiled) rather than in the cfg-gated `windows`/`linux` modules so its
/// unit tests run on every CI host, including macOS. The Windows (PE/COFF) and Linux (ELF)
/// loaders call these to turn an opaque "failed to load" into an actionable arch-mismatch
/// message, mirroring the macOS Mach-O diagnostic. Unused on macOS itself, hence the allow.
#[allow(dead_code)]
pub(crate) mod arch {
    /// The architecture this host binary was built for.
    #[cfg(target_arch = "aarch64")]
    pub(crate) const HOST_ARCH: &str = "arm64";
    #[cfg(target_arch = "x86_64")]
    pub(crate) const HOST_ARCH: &str = "x86_64";
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    pub(crate) const HOST_ARCH: &str = "unknown";

    // PE COFF machine values (always little-endian in the file).
    pub(crate) const PE_MACHINE_I386: u16 = 0x014c;
    pub(crate) const PE_MACHINE_AMD64: u16 = 0x8664;
    pub(crate) const PE_MACHINE_ARM64: u16 = 0xaa64;

    // ELF e_machine values.
    pub(crate) const ELF_MACHINE_386: u16 = 3;
    pub(crate) const ELF_MACHINE_X86_64: u16 = 62;
    pub(crate) const ELF_MACHINE_AARCH64: u16 = 183;

    /// Parse the COFF machine field of a PE image (a Windows `.dll`/`.vst3` binary).
    ///
    /// Layout: `MZ` at 0, `e_lfanew` (u32 LE) at 0x3C, `PE\0\0` at `e_lfanew`, machine (u16 LE)
    /// at `e_lfanew + 4`. Returns `None` if the magic is wrong or the data is truncated.
    pub(crate) fn detect_pe_machine(data: &[u8]) -> Option<u16> {
        if data.len() < 0x40 || data[0] != b'M' || data[1] != b'Z' {
            return None;
        }
        let e_lfanew =
            u32::from_le_bytes([data[0x3C], data[0x3D], data[0x3E], data[0x3F]]) as usize;
        let machine_off = e_lfanew.checked_add(4)?;
        if machine_off.checked_add(2)? > data.len() {
            return None;
        }
        if &data[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
            return None;
        }
        Some(u16::from_le_bytes([
            data[machine_off],
            data[machine_off + 1],
        ]))
    }

    /// Parse the `e_machine` field of an ELF image (a Linux `.so` binary).
    ///
    /// Honors `EI_DATA` (offset 5: 1 = little-endian, 2 = big-endian) when reading the u16 at
    /// offset 18. Returns `None` on bad magic or truncation.
    pub(crate) fn detect_elf_machine(data: &[u8]) -> Option<u16> {
        if data.len() < 20 || &data[0..4] != b"\x7fELF" {
            return None;
        }
        let bytes = [data[18], data[19]];
        Some(if data[5] == 2 {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        })
    }

    /// Human name for a PE machine value, or `None` if we don't map it.
    pub(crate) fn pe_machine_name(machine: u16) -> Option<&'static str> {
        match machine {
            PE_MACHINE_AMD64 => Some("x86_64"),
            PE_MACHINE_ARM64 => Some("arm64"),
            PE_MACHINE_I386 => Some("x86"),
            _ => None,
        }
    }

    /// Human name for an ELF machine value, or `None` if we don't map it.
    pub(crate) fn elf_machine_name(machine: u16) -> Option<&'static str> {
        match machine {
            ELF_MACHINE_X86_64 => Some("x86_64"),
            ELF_MACHINE_AARCH64 => Some("arm64"),
            ELF_MACHINE_386 => Some("x86"),
            _ => None,
        }
    }

    /// If `binary_arch` is known and differs from the host, an actionable sentence; else `None`.
    ///
    /// Returns `None` when the host arch is unknown (an exotic target we don't map) — we can't
    /// reliably claim a mismatch or suggest a "universal/unknown" build.
    pub(crate) fn mismatch_detail(kind: &str, binary_arch: Option<&str>) -> Option<String> {
        if HOST_ARCH == "unknown" {
            return None;
        }
        match binary_arch {
            Some(arch) if arch != HOST_ARCH => Some(format!(
                "Failed to load {kind}: plugin is {arch}-only but this host is {HOST_ARCH}. \
                 Use a universal/{HOST_ARCH} build of the plugin."
            )),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Minimal PE: MZ, e_lfanew=0x80, PE\0\0 header, machine at +4.
        fn pe_fixture(machine: u16) -> Vec<u8> {
            let mut data = vec![0u8; 0x88];
            data[0] = b'M';
            data[1] = b'Z';
            data[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
            data[0x80..0x84].copy_from_slice(b"PE\0\0");
            data[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
            data
        }

        /// Minimal ELF: magic, EI_DATA endianness at 5, e_machine at 18.
        fn elf_fixture(machine: u16, big_endian: bool) -> Vec<u8> {
            let mut data = vec![0u8; 20];
            data[0..4].copy_from_slice(b"\x7fELF");
            data[5] = if big_endian { 2 } else { 1 };
            let m = if big_endian {
                machine.to_be_bytes()
            } else {
                machine.to_le_bytes()
            };
            data[18..20].copy_from_slice(&m);
            data
        }

        #[test]
        fn pe_machine_detected() {
            assert_eq!(
                detect_pe_machine(&pe_fixture(PE_MACHINE_AMD64)),
                Some(PE_MACHINE_AMD64)
            );
            assert_eq!(
                detect_pe_machine(&pe_fixture(PE_MACHINE_ARM64)),
                Some(PE_MACHINE_ARM64)
            );
            assert_eq!(pe_machine_name(PE_MACHINE_AMD64), Some("x86_64"));
            assert_eq!(pe_machine_name(PE_MACHINE_ARM64), Some("arm64"));
        }

        #[test]
        fn pe_rejects_garbage_and_truncation() {
            assert_eq!(detect_pe_machine(&[0u8; 4]), None);
            assert_eq!(
                detect_pe_machine(b"not a pe file at all..............."),
                None
            );
            let mut short = pe_fixture(PE_MACHINE_AMD64);
            short.truncate(0x82); // cut off the machine field
            assert_eq!(detect_pe_machine(&short), None);
        }

        #[test]
        fn elf_machine_detected_both_endians() {
            assert_eq!(
                detect_elf_machine(&elf_fixture(ELF_MACHINE_X86_64, false)),
                Some(ELF_MACHINE_X86_64)
            );
            assert_eq!(
                detect_elf_machine(&elf_fixture(ELF_MACHINE_AARCH64, false)),
                Some(ELF_MACHINE_AARCH64)
            );
            // Big-endian EI_DATA must be honored.
            assert_eq!(
                detect_elf_machine(&elf_fixture(ELF_MACHINE_AARCH64, true)),
                Some(ELF_MACHINE_AARCH64)
            );
            assert_eq!(elf_machine_name(ELF_MACHINE_X86_64), Some("x86_64"));
            assert_eq!(elf_machine_name(ELF_MACHINE_AARCH64), Some("arm64"));
        }

        #[test]
        fn elf_rejects_garbage() {
            assert_eq!(detect_elf_machine(&[0u8; 8]), None);
            assert_eq!(detect_elf_machine(b"\x7fELF"), None); // too short
        }

        #[test]
        fn mismatch_detail_is_actionable() {
            // A binary whose arch differs from the host yields a message naming both.
            let other = if HOST_ARCH == "arm64" {
                "x86_64"
            } else {
                "arm64"
            };
            let msg =
                mismatch_detail("DLL", Some(other)).expect("mismatch should produce a detail");
            assert!(msg.contains(other), "names the binary arch: {msg}");
            assert!(msg.contains(HOST_ARCH), "names the host arch: {msg}");
            // Matching arch (or unknown) yields nothing.
            assert_eq!(mismatch_detail("DLL", Some(HOST_ARCH)), None);
            assert_eq!(mismatch_detail("DLL", None), None);
        }
    }
}

// Platform-specific modules
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub mod private_namespace;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod linux;
