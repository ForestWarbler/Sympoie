//! Plugin window management
//!
//! This module provides platform-specific window creation and management
//! for VST3 plugin GUIs.

use crate::error::{Error, Result};
use crate::plugin::Plugin;
use std::sync::{Arc, Mutex};

#[cfg(target_os = "macos")]
use objc2::{rc::Retained, MainThreadMarker, MainThreadOnly};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowOrderingMode, NSWindowStyleMask,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// A key typed into a plugin editor window that the host acts on instead of the plugin,
/// so transport control keeps working while an editor has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKey {
    /// The space bar, pressed without modifiers.
    Space,
}

/// Keys forwarded from plugin editors since the last call, oldest first.
///
/// Forwarding is implemented on macOS; other platforms return nothing. Call on the GUI thread.
pub fn take_editor_keys() -> Vec<EditorKey> {
    #[cfg(target_os = "macos")]
    {
        editor_keys::PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Called on the GUI thread whenever a key is forwarded, so an idle host can wake up and
/// call [`take_editor_keys`]. Replaces any previous waker.
pub fn set_editor_key_waker(waker: impl Fn() + 'static) {
    #[cfg(target_os = "macos")]
    editor_keys::WAKER.with(|slot| *slot.borrow_mut() = Some(Box::new(waker)));
    #[cfg(not(target_os = "macos"))]
    let _ = waker;
}

#[cfg(target_os = "macos")]
mod editor_keys {
    use super::EditorKey;
    use objc2::{rc::Retained, runtime::AnyObject, ClassType};
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSText, NSWindow};
    use objc2_foundation::NSObjectProtocol;
    use std::cell::RefCell;
    use std::ptr::NonNull;

    const SPACE_KEY_CODE: u16 = 49;

    thread_local! {
        /// Editor windows by address; only their key events are forwarded.
        static WINDOWS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
        pub(super) static PENDING: RefCell<Vec<EditorKey>> = const { RefCell::new(Vec::new()) };
        pub(super) static WAKER: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
        static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    }

    fn address(window: &NSWindow) -> usize {
        window as *const NSWindow as usize
    }

    pub(super) fn register(window: &NSWindow) {
        install();
        WINDOWS.with(|windows| windows.borrow_mut().push(address(window)));
    }

    pub(super) fn unregister(window: &NSWindow) {
        let address = address(window);
        WINDOWS.with(|windows| windows.borrow_mut().retain(|&known| known != address));
    }

    /// Whether the event should go to the host. Text typed into a native text field stays
    /// with the plugin.
    fn forwarded(event: &NSEvent) -> Option<EditorKey> {
        let window = event.window(objc2::MainThreadMarker::new()?)?;
        if !WINDOWS.with(|windows| windows.borrow().contains(&address(&window))) {
            return None;
        }
        let modifiers = event.modifierFlags()
            & (NSEventModifierFlags::Command
                | NSEventModifierFlags::Control
                | NSEventModifierFlags::Option
                | NSEventModifierFlags::Shift);
        if event.keyCode() != SPACE_KEY_CODE || !modifiers.is_empty() {
            return None;
        }
        let editing_text = window
            .firstResponder()
            .is_some_and(|responder| responder.isKindOfClass(NSText::class()));
        (!editing_text).then_some(EditorKey::Space)
    }

    fn install() {
        if MONITOR.with(|monitor| monitor.borrow().is_some()) {
            return;
        }
        let block = block2::RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit hands the monitor a live event for the duration of the call.
            let reference = unsafe { event.as_ref() };
            match forwarded(reference) {
                Some(key) => {
                    // Auto-repeat is swallowed so holding the bar toggles playback once.
                    if !reference.isARepeat() {
                        PENDING.with(|pending| pending.borrow_mut().push(key));
                        WAKER.with(|waker| {
                            if let Some(waker) = waker.borrow().as_ref() {
                                waker();
                            }
                        });
                    }
                    std::ptr::null_mut()
                }
                None => event.as_ptr(),
            }
        });
        // SAFETY: called on the main thread; the handler returns either the original event
        // or null, as the API requires. The monitor lives for the rest of the process.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
        };
        MONITOR.with(|slot| *slot.borrow_mut() = monitor);
    }
}

#[cfg(target_os = "windows")]
use winapi::{
    shared::minwindef::{LPARAM, LRESULT, UINT, WPARAM},
    shared::windef::{HWND, RECT},
    um::libloaderapi::GetModuleHandleW,
    um::winuser::{
        BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, GetActiveWindow,
        GetAncestor, IsIconic, IsWindow, LoadCursorW, RegisterClassExW, SetForegroundWindow,
        SetWindowPos, ShowWindow, UpdateWindow, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
        GA_ROOTOWNER, IDC_ARROW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SW_HIDE, SW_RESTORE,
        SW_SHOW, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, WM_CLOSE, WM_DPICHANGED, WNDCLASSEXW,
        WS_OVERLAPPEDWINDOW,
    },
};

#[cfg(any(test, target_os = "windows"))]
fn dpi_scale_factor(dpi: u32) -> Option<f32> {
    (dpi > 0).then_some(dpi as f32 / 96.0)
}

/// Editor windows whose window procedure has seen `WM_CLOSE`, keyed by `HWND` (as `usize`,
/// since `HWND` is a raw pointer and not `Send`).
///
/// `DefWindowProcW` answers `WM_CLOSE` by destroying the window, which would leave the plugin's
/// `IPlugView` attached to a freed `HWND`. [`plugin_window_proc`] records the request here
/// instead, so the host can tear the editor down in the right order.
#[cfg(target_os = "windows")]
fn close_requests() -> &'static Mutex<std::collections::HashSet<usize>> {
    static REQUESTS: std::sync::OnceLock<Mutex<std::collections::HashSet<usize>>> =
        std::sync::OnceLock::new();
    REQUESTS.get_or_init(Mutex::default)
}

/// Latest pending DPI for each editor window. The window procedure cannot call into the plugin:
/// `setContentScaleFactor` may synchronously enter arbitrary plugin/UI code, while callers often
/// hold the plugin mutex when Windows dispatches a message. The procedure records only the
/// newest DPI and [`PluginWindow::service_platform_events`] applies it after the callback returns.
#[cfg(target_os = "windows")]
fn dpi_changes() -> &'static Mutex<std::collections::HashMap<usize, u32>> {
    static CHANGES: std::sync::OnceLock<Mutex<std::collections::HashMap<usize, u32>>> =
        std::sync::OnceLock::new();
    CHANGES.get_or_init(Mutex::default)
}

#[cfg(target_os = "windows")]
fn dpi_from_wparam(wparam: WPARAM) -> Option<u32> {
    // WM_DPICHANGED packs the new X DPI into LOWORD and Y DPI into HIWORD. VST3 has one content
    // scale, and Windows normally reports both equally, so use X as Microsoft recommends.
    let dpi = (wparam & 0xffff) as u32;
    (dpi > 0).then_some(dpi)
}

/// Window procedure for plugin editor windows: record control-plane work and defer plugin calls
/// until after Windows returns from the callback.
///
/// # Safety
///
/// Called by the OS with the arguments of a window procedure; all it does with them is forward
/// them to `DefWindowProcW`.
#[cfg(target_os = "windows")]
unsafe extern "system" fn plugin_window_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CLOSE {
        if let Ok(mut requests) = close_requests().lock() {
            requests.insert(hwnd as usize);
        }
        return 0;
    }
    if msg == WM_DPICHANGED {
        // The suggested rectangle is valid only for this callback and must be applied
        // synchronously. Do that before taking any host mutex: SetWindowPos may dispatch more
        // window messages reentrantly.
        if let Some(suggested) = (lparam as *const RECT).as_ref() {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        if let Some(dpi) = dpi_from_wparam(wparam) {
            if let Ok(mut changes) = dpi_changes().lock() {
                changes.insert(hwnd as usize, dpi);
            }
        }
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// An X11 window (connection + window id) backing a plugin editor on Linux.
///
/// Ported from the khremeviuc1004 fork's XCB implementation.
#[cfg(target_os = "linux")]
struct XcbWindowState {
    connection: xcb::Connection,
    window: xcb::x::Window,
    root: xcb::x::Window,
    active_window_atom: Option<xcb::x::Atom>,
}

/// The host application's native window that owns its plugin editors.
///
/// Capture it once from the main host window, then clone it when opening editors. This avoids
/// accidentally making a floating parameter window or another editor the owner. On macOS this
/// retains the NSWindow and is restricted to the main thread by its native type.
#[derive(Clone)]
pub struct PluginWindowOwner {
    #[cfg(target_os = "macos")]
    window: Retained<NSWindow>,
    #[cfg(target_os = "windows")]
    window: HWND,
    #[cfg(target_os = "linux")]
    window: u32,
}

impl PluginWindowOwner {
    /// Capture a native host window from a safely borrowed window handle.
    ///
    /// Requires the `native-window-owner` feature. Call on the window's GUI thread while the
    /// host window is alive. AppKit handles identify an NSView; its containing NSWindow is
    /// retained here. X11 editors require an Xlib/XCB host window rather than a Wayland handle.
    #[cfg(feature = "native-window-owner")]
    pub fn from_window_handle(handle: raw_window_handle::WindowHandle<'_>) -> Result<Self> {
        #[cfg(target_os = "macos")]
        if let raw_window_handle::RawWindowHandle::AppKit(handle) = handle.as_raw() {
            let _mtm = MainThreadMarker::new().ok_or_else(|| {
                Error::Other("plugin window owner must be captured on the main thread".into())
            })?;
            // SAFETY: WindowHandle's borrow guarantees a live NSView for this call. Retaining
            // its window extends the AppKit object's lifetime without retaining a raw pointer.
            let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
            return view.window().map(|window| Self { window }).ok_or_else(|| {
                Error::Other("host NSView is not attached to a native window".into())
            });
        }
        #[cfg(target_os = "windows")]
        if let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() {
            let window = handle.hwnd.get() as HWND;
            // SAFETY: Windows validates the supplied HWND without dereferencing it in Rust.
            if unsafe { IsWindow(window) } != 0 {
                return Ok(Self { window });
            }
            return Err(Error::Other("host window is no longer valid".into()));
        }
        #[cfg(target_os = "linux")]
        match handle.as_raw() {
            raw_window_handle::RawWindowHandle::Xcb(handle) => {
                return Ok(Self {
                    window: handle.window.get(),
                });
            }
            raw_window_handle::RawWindowHandle::Xlib(handle) => {
                return Ok(Self {
                    window: handle.window as u32,
                });
            }
            _ => {}
        }
        let _ = handle;
        Err(Error::Other(
            "unsupported plugin window owner handle".into(),
        ))
    }

    /// Attach the application's other visible windows titled exactly `title` above this
    /// owner, the way plugin editors are attached: they stay in front of the host window when
    /// it is clicked, without floating over other applications. Windows that already have a
    /// parent are left alone, so calling this every frame is cheap. Returns how many windows
    /// were newly attached.
    ///
    /// Implemented on macOS; other platforms attach nothing. Call on the GUI thread.
    pub fn adopt_windows_titled(&self, title: &str) -> usize {
        #[cfg(target_os = "macos")]
        {
            let Some(mtm) = MainThreadMarker::new() else {
                return 0;
            };
            let mut adopted = 0;
            for window in NSApplication::sharedApplication(mtm).windows().iter() {
                if Retained::as_ptr(&window) == Retained::as_ptr(&self.window)
                    || window.parentWindow().is_some()
                    || !window.isVisible()
                    || window.isMiniaturized()
                    || window.title().to_string() != title
                {
                    continue;
                }
                // SAFETY: both windows are live on the main thread and the candidate has no
                // parent, so attaching it cannot create an ownership cycle.
                unsafe {
                    self.window
                        .addChildWindow_ordered(&window, NSWindowOrderingMode::Above)
                };
                adopted += 1;
            }
            adopted
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = title;
            0
        }
    }
}

/// A plugin window that manages the native window and plugin editor lifecycle
pub struct PluginWindow {
    plugin: Arc<Mutex<Plugin>>,
    owner: Option<PluginWindowOwner>,
    /// Host selection hides a window without closing its plugin view.
    hidden_by_host: bool,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    minimized_before_hide: bool,
    /// AppKit removes child ownership when a window is ordered out. Remember and reattach it
    /// when the selected track becomes visible again.
    #[cfg(target_os = "macos")]
    hidden_parent: Option<Retained<NSWindow>>,
    #[cfg(target_os = "macos")]
    native_window: Option<Retained<NSWindow>>,
    /// The view the plugin attached its editor into. Kept so a plugin-initiated resize can
    /// grow the container along with the window.
    #[cfg(target_os = "macos")]
    container_view: Option<Retained<NSView>>,
    #[cfg(target_os = "windows")]
    native_window: Option<HWND>,
    #[cfg(target_os = "linux")]
    native_window: Option<XcbWindowState>,
    #[cfg(target_os = "android")]
    native_window: Option<()>,
}

impl PluginWindow {
    /// Create a new plugin window for the given plugin
    pub fn new(plugin: Arc<Mutex<Plugin>>) -> Self {
        Self {
            plugin,
            owner: None,
            hidden_by_host: false,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            minimized_before_hide: false,
            #[cfg(target_os = "macos")]
            hidden_parent: None,
            #[cfg(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "linux",
                target_os = "android"
            ))]
            native_window: None,
            #[cfg(target_os = "macos")]
            container_view: None,
        }
    }

    /// Set the main host window that this editor must remain above.
    ///
    /// Configure this before opening the editor. Without an explicit owner, macOS and Windows
    /// fall back to the current application window; X11 editors have no transient owner.
    pub fn set_owner(&mut self, owner: PluginWindowOwner) -> Result<()> {
        if self.native_window.is_some() {
            return Err(Error::Other(
                "set the plugin editor owner before opening its window".into(),
            ));
        }
        self.owner = Some(owner);
        Ok(())
    }

    /// Open the plugin window, or restore and focus it if it is already open.
    ///
    /// On macOS and Windows, the editor belongs to the current application window and stays
    /// above that owner without becoming a globally topmost window. Call this on the GUI thread
    /// in response to an explicit user action, while the host's window is active.
    pub fn open(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            Error::Other("plugin editor window must be opened on the main thread".to_string())
        })?;

        if self.is_open() {
            return self.focus();
        }

        // Check if plugin has editor
        let has_editor = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .has_editor();
        if !has_editor {
            return Err(Error::Other(
                "Plugin does not have a GUI editor".to_string(),
            ));
        }

        // Close existing window if any. Keyed on the handle rather than `is_open()`, which also
        // reports `false` for a window the user already dismissed — that one still needs closing.
        if self.native_window.is_some() {
            self.close();
        }

        // Get plugin info for window title
        let plugin_info = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .info()
            .clone();

        // Try to get editor size
        let (width, height) = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_editor_size()
            .unwrap_or((800, 600));

        // Create native window
        #[cfg(target_os = "macos")]
        {
            // Capture the host before showing the editor, which can itself become the main
            // window. If the user opens another editor from one of our child windows, attach
            // the new editor to the common host rather than building a chain of editors.
            let application = NSApplication::sharedApplication(mtm);
            let owner = match self.owner.as_ref() {
                Some(owner) => Some(owner.window.clone()),
                None => {
                    let mut owner = application.mainWindow().or_else(|| application.keyWindow());
                    while let Some(parent) = owner.as_ref().and_then(|window| window.parentWindow())
                    {
                        owner = Some(parent);
                    }
                    owner
                }
            };

            let frame = NSRect::new(
                NSPoint::new(100.0, 100.0),
                NSSize::new(width as f64, height as f64),
            );
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable;

            // SAFETY: standard AppKit window/view construction on the main thread.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    frame,
                    style,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };

            // Programmatic NSWindows default to `releasedWhenClosed = YES`; closing one would
            // then release it while our `Retained<NSWindow>` also releases on drop — a
            // double-free that crashes on close. We own the lifetime, so opt out.
            // SAFETY: standard AppKit setter on the main thread.
            unsafe { window.setReleasedWhenClosed(false) };

            let title = NSString::from_str(&format!("{} - VST3", plugin_info.name));
            window.setTitle(&title);
            // Permit the editor alongside its host in a full-screen Space. The child-window
            // relationship controls stacking; no floating/global window level is required.
            window.setCollectionBehavior(
                window.collectionBehavior() | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );

            // A container view, sized to the editor, that the plugin attaches its view into.
            let container_frame = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width as f64, height as f64),
            );
            let container_view = NSView::initWithFrame(NSView::alloc(mtm), container_frame);
            if let Some(content_view) = window.contentView() {
                content_view.addSubview(&container_view);
            }

            // Hand the plugin the container NSView to embed its editor in.
            // SAFETY: `container_view` is a live NSView this `PluginWindow` keeps alive (via
            // the retained window it was added to) for as long as the editor is attached —
            // `close()` detaches the editor before dropping the window.
            let window_handle = unsafe {
                crate::plugin::WindowHandle::from_nsview(
                    Retained::as_ptr(&container_view) as *mut std::ffi::c_void
                )
            };
            self.plugin
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .open_editor(window_handle)?;

            // Match the window to the editor size, then attach it above the host. AppKit keeps
            // this relationship when the user brings the main DAW window forward, while other
            // applications can still cover both windows normally.
            window.setContentSize(container_frame.size);
            window.center();
            if let Some(owner) = owner.as_ref() {
                // SAFETY: both windows are live on the main thread. The newly created window
                // has no parent or children, so this cannot create an ownership cycle.
                unsafe { owner.addChildWindow_ordered(&window, NSWindowOrderingMode::Above) };
            }
            window.makeKeyAndOrderFront(None);
            editor_keys::register(&window);

            self.native_window = Some(window);
            self.container_view = Some(container_view);
        }

        #[cfg(target_os = "windows")]
        {
            unsafe {
                use std::mem;
                use std::ptr;

                // Register window class if not already registered
                let class_name = "VST3PluginWindow\0".encode_utf16().collect::<Vec<u16>>();
                let mut wc: WNDCLASSEXW = mem::zeroed();
                wc.cbSize = mem::size_of::<WNDCLASSEXW>() as UINT;
                wc.style = CS_HREDRAW | CS_VREDRAW;
                wc.lpfnWndProc = Some(plugin_window_proc);
                wc.hInstance = GetModuleHandleW(ptr::null());
                wc.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
                wc.lpszClassName = class_name.as_ptr();

                // Try to register, ignore if already registered
                RegisterClassExW(&wc);

                // Create window
                let window_title = format!("{} - VST3\0", plugin_info.name);
                let window_name = window_title.encode_utf16().collect::<Vec<u16>>();

                // Calculate window size including borders
                let mut rect = RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                };

                winapi::um::winuser::AdjustWindowRectEx(
                    &mut rect,
                    WS_OVERLAPPEDWINDOW,
                    0, // No menu
                    0, // No extended style
                );

                let window_width = rect.right - rect.left;
                let window_height = rect.bottom - rect.top;

                // Owned top-level windows stay above their owner and hide with it when it is
                // minimized, without covering other applications like HWND_TOPMOST would.
                // GetActiveWindow is limited to this GUI thread's windows.
                let owner = if let Some(owner) = self.owner.as_ref() {
                    if IsWindow(owner.window) == 0 {
                        return Err(Error::Other(
                            "plugin editor owner is no longer valid".into(),
                        ));
                    }
                    owner.window
                } else {
                    let active = GetActiveWindow();
                    if active.is_null() {
                        ptr::null_mut()
                    } else {
                        GetAncestor(active, GA_ROOTOWNER)
                    }
                };
                let hwnd = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    window_name.as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    window_width,
                    window_height,
                    owner,
                    ptr::null_mut(),
                    GetModuleHandleW(ptr::null()),
                    ptr::null_mut(),
                );

                if hwnd.is_null() {
                    return Err(Error::Other("Failed to create native window".to_string()));
                }

                // Try to open plugin editor.
                // SAFETY: `hwnd` was just created above and null-checked; it is destroyed only
                // after the editor is detached (in `close()`, or the error arm below).
                let window_handle =
                    crate::plugin::WindowHandle::from_hwnd(hwnd as *mut std::ffi::c_void);
                let mut plugin = self.plugin.lock().unwrap_or_else(|p| p.into_inner());
                let dpi = winapi::um::winuser::GetDpiForWindow(hwnd);
                if let Some(scale_factor) = dpi_scale_factor(dpi) {
                    if let Err(error) = plugin.set_editor_scale_factor(scale_factor) {
                        drop(plugin);
                        DestroyWindow(hwnd);
                        return Err(error);
                    }
                }
                match plugin.open_editor(window_handle) {
                    Ok(()) => {
                        drop(plugin);
                        ShowWindow(hwnd, SW_SHOW);
                        UpdateWindow(hwnd);
                        self.native_window = Some(hwnd);
                    }
                    Err(e) => {
                        drop(plugin);
                        DestroyWindow(hwnd);
                        return Err(e);
                    }
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            use xcb::Xid;

            // Create an X11 window via XCB and embed the plugin editor into it using the
            // VST3 X11EmbedWindowID platform type (handled in plugin_impl::open_editor).
            let (connection, screen_number) = xcb::Connection::connect(None)
                .map_err(|e| Error::Other(format!("Failed to connect to X server: {e}")))?;
            let setup = connection.get_setup();
            let screen = setup
                .roots()
                .nth(screen_number as usize)
                .ok_or_else(|| Error::Other("No X11 screen found".to_string()))?;
            let root = screen.root();
            let window = connection.generate_id();

            connection
                .send_and_check_request(&xcb::x::CreateWindow {
                    depth: xcb::x::COPY_FROM_PARENT as u8,
                    wid: window,
                    parent: screen.root(),
                    x: 0,
                    y: 0,
                    width: width as u16,
                    height: height as u16,
                    border_width: 0,
                    class: xcb::x::WindowClass::InputOutput,
                    visual: screen.root_visual(),
                    value_list: &[
                        xcb::x::Cw::BackPixel(screen.white_pixel()),
                        xcb::x::Cw::EventMask(
                            xcb::x::EventMask::EXPOSURE | xcb::x::EventMask::KEY_PRESS,
                        ),
                    ],
                })
                .map_err(|e| Error::Other(format!("Failed to create X11 window: {e}")))?;

            // Window title.
            let title = format!("{} - VST3", plugin_info.name);
            connection.send_request(&xcb::x::ChangeProperty {
                mode: xcb::x::PropMode::Replace,
                window,
                property: xcb::x::ATOM_WM_NAME,
                r#type: xcb::x::ATOM_STRING,
                data: title.as_bytes(),
            });
            if let Some(owner) = self.owner.as_ref() {
                // ICCCM transient ownership lets the window manager keep the editor above its
                // host and minimize them together, without asking for global ABOVE state.
                connection.send_request(&xcb::x::ChangeProperty {
                    mode: xcb::x::PropMode::Replace,
                    window,
                    property: xcb::x::ATOM_WM_TRANSIENT_FOR,
                    r#type: xcb::x::ATOM_WINDOW,
                    data: &[owner.window],
                });
            }

            // Show the window, then attach the plugin editor to its X11 id.
            connection.send_request(&xcb::x::MapWindow { window });
            let _ = connection.flush();

            let handle = crate::plugin::WindowHandle::from_x11(window.resource_id());
            self.plugin
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .open_editor(handle)?;

            let active_window_atom = connection
                .wait_for_reply(connection.send_request(&xcb::x::InternAtom {
                    only_if_exists: false,
                    name: b"_NET_ACTIVE_WINDOW",
                }))
                .ok()
                .map(|reply| reply.atom());
            self.native_window = Some(XcbWindowState {
                connection,
                window,
                root,
                active_window_atom,
            });
        }

        #[cfg(target_os = "android")]
        {
            return Err(Error::Other(
                "PluginWindow::open() is not supported on Android".to_string(),
            ));
        }

        Ok(())
    }

    /// Restore and focus an already-open editor without recreating its plugin view.
    ///
    /// Call only for an explicit user request on the GUI thread, never on every UI update:
    /// normal stacking is maintained by native ownership, so servicing an editor does not steal
    /// focus from the arrangement, another editor, or another application.
    pub fn focus(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        let _mtm = MainThreadMarker::new().ok_or_else(|| {
            Error::Other("plugin editor window must be focused on the main thread".to_string())
        })?;
        if !self.is_open() {
            return Err(Error::Other("plugin editor window is not open".to_string()));
        }
        // Explicit Open Editor overrides a remembered minimized state. Automatic track
        // restoration preserves it, but focus must not schedule minimization just before
        // trying to restore the same window.
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.minimized_before_hide = false;
        }
        self.set_visible(true)?;

        #[cfg(target_os = "macos")]
        if let Some(window) = self.native_window.as_ref() {
            if let Some(owner) = window.parentWindow() {
                if owner.isMiniaturized() {
                    owner.deminiaturize(None);
                }
            }
            if window.isMiniaturized() {
                window.deminiaturize(None);
            }
            window.makeKeyAndOrderFront(None);
        }

        #[cfg(target_os = "windows")]
        if let Some(hwnd) = self.native_window {
            // SAFETY: this live handle belongs to this PluginWindow and GUI thread. None of
            // these operations destroys it or enters plugin code while holding its mutex.
            unsafe {
                let owner = GetAncestor(hwnd, GA_ROOTOWNER);
                if !owner.is_null() && IsIconic(owner) != 0 {
                    ShowWindow(owner, SW_RESTORE);
                }
                ShowWindow(
                    hwnd,
                    if IsIconic(hwnd) != 0 {
                        SW_RESTORE
                    } else {
                        SW_SHOW
                    },
                );
                BringWindowToTop(hwnd);
                SetForegroundWindow(hwnd);
            }
        }

        #[cfg(target_os = "linux")]
        if let Some(state) = self.native_window.as_ref() {
            state.connection.send_request(&xcb::x::MapWindow {
                window: state.window,
            });
            state.connection.send_request(&xcb::x::ConfigureWindow {
                window: state.window,
                value_list: &[xcb::x::ConfigWindow::StackMode(xcb::x::StackMode::Above)],
            });
            if let Some(atom) = state.active_window_atom {
                // Let the window manager restore/activate a minimized editor. SetInputFocus
                // immediately after MapWindow can fail because mapping is redirected to the
                // WM and the window is not yet viewable when the focus request reaches X11.
                let event = xcb::x::ClientMessageEvent::new(
                    state.window,
                    atom,
                    xcb::x::ClientMessageData::Data32([1, xcb::x::CURRENT_TIME, 0, 0, 0]),
                );
                state.connection.send_request(&xcb::x::SendEvent {
                    propagate: false,
                    destination: xcb::x::SendEventDest::Window(state.root),
                    event_mask: xcb::x::EventMask::SUBSTRUCTURE_REDIRECT
                        | xcb::x::EventMask::SUBSTRUCTURE_NOTIFY,
                    event: &event,
                });
            }
            state
                .connection
                .flush()
                .map_err(|error| Error::Other(format!("Failed to raise plugin editor: {error}")))?;
        }

        Ok(())
    }

    /// Temporarily hide or reveal an editor while preserving its attached plugin view.
    ///
    /// Revealing never requests keyboard focus or activates the application. On macOS and
    /// Windows it preserves an editor the user minimized; X11 mapping/minimization follows the
    /// window manager's policy. Unlike closing, hiding keeps [`Self::is_open`] true. A window
    /// already closed by the user stays closed; showing it here cannot resurrect it. Use
    /// [`Self::open`] or [`Self::focus`] for an explicit user request to open/focus an editor.
    pub fn set_visible(&mut self, visible: bool) -> Result<()> {
        #[cfg(target_os = "macos")]
        let _mtm = MainThreadMarker::new().ok_or_else(|| {
            Error::Other("plugin editor visibility must change on the main thread".into())
        })?;
        if !self.is_open() || visible == !self.hidden_by_host {
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        if let Some(window) = self.native_window.as_ref() {
            if visible {
                if let Some(owner) = self.hidden_parent.take() {
                    // SAFETY: the live main-thread child was detached from this exact owner
                    // when hidden, and cannot have created an ownership cycle while hidden.
                    unsafe { owner.addChildWindow_ordered(window, NSWindowOrderingMode::Above) };
                }
                if self.minimized_before_hide {
                    if !window.isMiniaturized() {
                        window.miniaturize(None);
                    }
                } else if !window.isMiniaturized()
                    && !window
                        .parentWindow()
                        .is_some_and(|owner| owner.isMiniaturized())
                {
                    window.orderFront(None);
                }
            } else {
                self.minimized_before_hide = window.isMiniaturized();
                self.hidden_parent = window.parentWindow();
                if let Some(owner) = self.hidden_parent.as_ref() {
                    owner.removeChildWindow(window);
                }
                window.orderOut(None);
            }
        }

        #[cfg(target_os = "windows")]
        if let Some(hwnd) = self.native_window {
            // SAFETY: the editor owns this live HWND and changes visibility on its GUI thread.
            // SW_*NOACTIVATE leaves keyboard focus with the DAW during a track switch.
            unsafe {
                if visible {
                    ShowWindow(
                        hwnd,
                        if self.minimized_before_hide {
                            SW_SHOWMINNOACTIVE
                        } else {
                            SW_SHOWNOACTIVATE
                        },
                    );
                } else {
                    self.minimized_before_hide = IsIconic(hwnd) != 0;
                    ShowWindow(hwnd, SW_HIDE);
                }
            }
        }

        #[cfg(target_os = "linux")]
        if let Some(state) = self.native_window.as_ref() {
            if visible {
                state.connection.send_request(&xcb::x::MapWindow {
                    window: state.window,
                });
            } else {
                state.connection.send_request(&xcb::x::UnmapWindow {
                    window: state.window,
                });
            }
            state.connection.flush().map_err(|error| {
                Error::Other(format!(
                    "Failed to change plugin editor visibility: {error}"
                ))
            })?;
        }

        self.hidden_by_host = !visible;
        Ok(())
    }

    /// Pump the editor's window-level traffic that has to cross into the plugin.
    ///
    /// **Call this from your UI event loop, once per frame, while the window is open.** It
    /// handles:
    ///
    /// - **Plugin-initiated resizes.** A resizable editor (VSTGUI zoom, a "big view" toggle)
    ///   asks the host to resize through `IPlugFrame::resizeView`. This applies the request to
    ///   the native window, so the editor is not clipped by a window that never grew.
    /// - **Windows DPI changes.** `WM_DPICHANGED` applies its suggested native rectangle
    ///   immediately in the window procedure and queues the new content scale for here; making
    ///   the COM call outside the window procedure prevents reentrant deadlocks on the plugin
    ///   mutex.
    /// - **Linux editor callbacks.** VSTGUI's registered run-loop timers and file descriptors
    ///   are serviced on this GUI thread so plugin displays animate and respond to input.
    ///
    /// Deliberately non-blocking: if the plugin mutex is held elsewhere (the audio callback
    /// holds it per block) this returns without doing anything, and the pending work is picked
    /// up on the next call.
    ///
    /// [`Self::is_open`] and [`Self::closed_by_user`] do *not* run this — they stay lock-free
    /// so they are safe to call while holding the plugin lock.
    pub fn service_platform_events(&self) -> Result<()> {
        if self.native_window.is_none() {
            return Ok(());
        }
        let Some(mut plugin) = self.try_lock_plugin() else {
            return Ok(());
        };

        let scale_result = self
            .take_pending_scale_factor()
            .map(|factor| plugin.set_editor_scale_factor(factor));
        let resize = plugin.take_editor_resize_request();
        // Linux VSTGUI timers and file descriptors are registered with the host's IRunLoop.
        // Pumping them on the GUI thread keeps animated displays and editor input responsive.
        // This is a no-op on platforms whose native application loop supplies those callbacks.
        plugin.service_run_loop();

        // Released before touching the native window: on Windows, resizing it may synchronously
        // dispatch window messages back into host code that wants this same lock.
        drop(plugin);

        if let Some((width, height)) = resize {
            self.resize_native_window(width, height);
        }
        match scale_result {
            Some(Err(error)) => Err(error),
            _ => Ok(()),
        }
    }

    /// Take the plugin lock without blocking, recovering a lock poisoned by an unrelated panic
    /// (a poisoned mutex is permanent, and treating it as failure would stop servicing the
    /// editor for the rest of the session).
    fn try_lock_plugin(&self) -> Option<std::sync::MutexGuard<'_, Plugin>> {
        match self.plugin.try_lock() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::Poisoned(poison)) => Some(poison.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    /// The newest DPI the window procedure recorded for this window, as a VST3 content scale.
    #[cfg(target_os = "windows")]
    fn take_pending_scale_factor(&self) -> Option<f32> {
        let hwnd = self.native_window?;
        dpi_changes()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&(hwnd as usize))
            .and_then(dpi_scale_factor)
    }

    /// Only Windows reports DPI changes through the window procedure.
    #[cfg(not(target_os = "windows"))]
    fn take_pending_scale_factor(&self) -> Option<f32> {
        None
    }

    /// Resize the native window so it fits an editor of `width` x `height` pixels.
    ///
    /// The plugin has already resized its own view (the host answered `resizeView` with
    /// `onSize`); this catches the window up.
    fn resize_native_window(&self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        log::debug!("editor asked the host to resize its window to {width}x{height}");

        #[cfg(target_os = "macos")]
        {
            let Some(window) = self.native_window.as_ref() else {
                return;
            };
            let size = NSSize::new(width as f64, height as f64);
            if let Some(container) = self.container_view.as_ref() {
                container.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), size));
            }
            window.setContentSize(size);
        }

        #[cfg(target_os = "windows")]
        {
            let Some(hwnd) = self.native_window else {
                return;
            };
            // The plugin sizes its *client* area; grow the frame by the window's chrome, the
            // same way `open()` sizes the window to the editor in the first place.
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            unsafe {
                winapi::um::winuser::AdjustWindowRectEx(&mut rect, WS_OVERLAPPEDWINDOW, 0, 0);
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }

        #[cfg(target_os = "linux")]
        {
            let Some(state) = self.native_window.as_ref() else {
                return;
            };
            state.connection.send_request(&xcb::x::ConfigureWindow {
                window: state.window,
                value_list: &[
                    xcb::x::ConfigWindow::Width(width as u32),
                    xcb::x::ConfigWindow::Height(height as u32),
                ],
            });
            let _ = state.connection.flush();
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        let _ = (width, height);
    }

    /// Close the plugin window
    pub fn close(&mut self) {
        // Detach the plugin editor first — it must never outlive the window it is embedded in.
        // A poisoned lock (an earlier panic on the audio thread) is recovered rather than
        // skipped, exactly as every other lock site here does: skipping it would destroy the
        // native window with the plugin's view still attached to it.
        let _ = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .close_editor();

        // Then close the native window
        #[cfg(target_os = "macos")]
        {
            self.hidden_parent = None;
            self.container_view = None;
            if let Some(window) = self.native_window.take() {
                editor_keys::unregister(&window);
                if let Some(owner) = window.parentWindow() {
                    owner.removeChildWindow(&window);
                }
                window.close();
            }
        }

        #[cfg(target_os = "windows")]
        {
            if let Some(hwnd) = self.native_window.take() {
                if let Ok(mut requests) = close_requests().lock() {
                    requests.remove(&(hwnd as usize));
                }
                if let Ok(mut changes) = dpi_changes().lock() {
                    changes.remove(&(hwnd as usize));
                }
                unsafe {
                    DestroyWindow(hwnd);
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Some(state) = self.native_window.take() {
                state.connection.send_request(&xcb::x::UnmapWindow {
                    window: state.window,
                });
                state.connection.send_request(&xcb::x::DestroyWindow {
                    window: state.window,
                });
                let _ = state.connection.flush();
            }
        }

        #[cfg(target_os = "android")]
        {
            let _ = self.native_window.take();
        }
        self.hidden_by_host = false;
    }

    /// Check if the window is currently open.
    ///
    /// Reports `false` once the user has dismissed the window themselves, even though the
    /// handle is still around — see [`Self::closed_by_user`]. An editor hidden through
    /// [`Self::set_visible`] remains open and attached.
    ///
    /// Reads native window state only: it never takes the plugin lock, so it is safe to call
    /// while holding it. Pair it with [`Self::service_platform_events`], which does the work
    /// that has to reach the plugin.
    pub fn is_open(&self) -> bool {
        self.native_window.is_some() && !self.native_window_dismissed()
    }

    /// Whether the user closed the window themselves, through its title-bar close button.
    ///
    /// Neither the plugin nor the host is told when that happens, so a host that tracks "the
    /// editor is open" in its own UI should poll this each frame and drop or
    /// [`close`](Self::close) the window when it reports `true`. Otherwise the editor stays
    /// attached to a window that is gone from the screen.
    ///
    /// Cheap and lock-free in the same sense as [`Self::is_open`]: native window state only,
    /// never the plugin lock.
    pub fn closed_by_user(&self) -> bool {
        self.native_window_dismissed()
    }

    /// Platform probe behind [`Self::closed_by_user`].
    #[cfg(target_os = "macos")]
    fn native_window_dismissed(&self) -> bool {
        if self.hidden_by_host {
            return false;
        }
        // Even reading an NSWindow is an AppKit operation. Avoid touching it off the main
        // thread; the owner will observe the actual state on its next GUI update.
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let Some(window) = self.native_window.as_ref() else {
            return false;
        };
        // A closed window is ordered out — but so is a miniaturized one, and so is every window
        // of a hidden application. Neither of those means the user dismissed the editor.
        if window.isVisible() || window.isMiniaturized() {
            return false;
        }
        // AppKit orders child windows out when their owner is minimized. That is not an editor
        // close request: keep the view attached so it reappears with the host when restored.
        if window
            .parentWindow()
            .is_some_and(|owner| owner.isMiniaturized())
        {
            return false;
        }
        !NSApplication::sharedApplication(mtm).isHidden()
    }

    /// Platform probe behind [`Self::closed_by_user`].
    #[cfg(target_os = "windows")]
    fn native_window_dismissed(&self) -> bool {
        let Some(hwnd) = self.native_window else {
            return false;
        };
        close_requests()
            .lock()
            .is_ok_and(|requests| requests.contains(&(hwnd as usize)))
    }

    /// Platform probe behind [`Self::closed_by_user`]. X11 and Android editor windows carry no
    /// close affordance of their own, so there is nothing to observe.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn native_window_dismissed(&self) -> bool {
        false
    }
}

impl Drop for PluginWindow {
    fn drop(&mut self) {
        self.close();
    }
}

/// Builder for creating plugin windows with egui integration
#[cfg(feature = "egui-widgets")]
pub struct PluginWindowBuilder {
    plugin: Arc<Mutex<Plugin>>,
}

#[cfg(feature = "egui-widgets")]
impl PluginWindowBuilder {
    /// Create a new builder for the given plugin
    pub fn new(plugin: Arc<Mutex<Plugin>>) -> Self {
        Self { plugin }
    }

    /// Build and open a standalone plugin window
    pub fn open_standalone(&self) -> Result<PluginWindow> {
        let mut window = PluginWindow::new(self.plugin.clone());
        window.open()?;
        Ok(window)
    }
}

#[cfg(test)]
mod tests {
    use super::dpi_scale_factor;

    #[test]
    fn windows_dpi_converts_to_vst_content_scale() {
        assert_eq!(dpi_scale_factor(0), None);
        assert_eq!(dpi_scale_factor(96), Some(1.0));
        assert_eq!(dpi_scale_factor(120), Some(1.25));
        assert_eq!(dpi_scale_factor(144), Some(1.5));
        assert_eq!(dpi_scale_factor(192), Some(2.0));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn wm_dpi_wparam_uses_horizontal_dpi() {
        let packed = (192usize << 16) | 144;
        assert_eq!(super::dpi_from_wparam(packed), Some(144));
        assert_eq!(super::dpi_from_wparam(0), None);
    }
}
