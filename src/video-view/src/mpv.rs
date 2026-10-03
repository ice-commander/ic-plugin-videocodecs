//! Opened at run time, not linked: the web host loads the plugin without libmpv, and a carried copy comes first.

use std::ffi::CString;
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;

type Handle = *mut c_void;
type Context = *mut c_void;

const FORMAT_FLAG: c_int = 3;
const FORMAT_DOUBLE: c_int = 5;

const PARAM_INVALID: c_int = 0;
const PARAM_API_TYPE: c_int = 1;
const PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const PARAM_OPENGL_FBO: c_int = 3;
const PARAM_FLIP_Y: c_int = 4;

#[repr(C)]
struct Param {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct GlInit {
    get_proc_address: extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    ctx: *mut c_void,
}

#[repr(C)]
struct Fbo {
    fbo: c_int,
    width: c_int,
    height: c_int,
    internal_format: c_int,
}

/// Called from whichever thread decoded, which is never the one that draws.
pub type Wake = extern "C" fn(*mut c_void);

struct Api {
    create: extern "C" fn() -> Handle,
    initialize: extern "C" fn(Handle) -> c_int,
    terminate_destroy: extern "C" fn(Handle),
    set_option_string: extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_property: extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    get_property: extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    command: extern "C" fn(Handle, *mut *const c_char) -> c_int,
    render_create: extern "C" fn(*mut Context, Handle, *mut Param) -> c_int,
    render_set_update_callback: extern "C" fn(Context, Wake, *mut c_void),
    render_render: extern "C" fn(Context, *mut Param) -> c_int,
    render_free: extern "C" fn(Context),
}

static WHENCE: OnceLock<String> = OnceLock::new();

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(load).as_ref()
}

pub fn found() -> bool {
    api().is_some()
}

pub fn whence() -> &'static str {
    WHENCE.get().map(String::as_str).unwrap_or("")
}

fn load() -> Option<Api> {
    #[cfg(target_os = "linux")]
    if let Some(beside) = own_folder() {
        fall_back(&beside.join(CODECS).join(FALLBACK));
    }
    for at in candidates() {
        let Some(library) = open(&at) else {
            continue;
        };
        let Some(api) = bind(library) else {
            continue;
        };
        let _ = WHENCE.set(at);
        return Some(api);
    }
    None
}

fn candidates() -> Vec<String> {
    let mut found = Vec::new();
    if let Ok(named) = std::env::var("IC_VIDEO_MPV") {
        if !named.is_empty() {
            found.push(named);
        }
    }
    if let Some(beside) = own_folder() {
        for name in NAMES {
            found.push(beside.join(CODECS).join(name).to_string_lossy().into_owned());
            found.push(beside.join(name).to_string_lossy().into_owned());
        }
    }
    found.extend(NAMES.iter().map(|name| (*name).to_string()));
    found
}

/// A subfolder because the host tries every library in the plugin folder as a plugin.
pub const CODECS: &str = "video-codecs";

/// Inside CODECS: what libmpv links on Linux that a desktop may lack.
#[cfg(target_os = "linux")]
const FALLBACK: &str = "fallback";

/// Each of these is loaded only where the system has none of its own: a system copy matches the system's
/// drivers, which matters for libva. Rounds, because one may need another that is not loaded yet.
#[cfg(target_os = "linux")]
fn fall_back(folder: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut waiting: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    waiting.sort();
    loop {
        let before = waiting.len();
        waiting.retain(|path| {
            let Some(soname) = path.file_name().and_then(|name| name.to_str()) else {
                return false;
            };
            !open_global(soname) && !open_global(&path.to_string_lossy())
        });
        if waiting.is_empty() || waiting.len() == before {
            break;
        }
    }
}

#[cfg(target_os = "macos")]
const NAMES: &[&str] = &["libmpv.2.dylib", "libmpv.dylib"];
#[cfg(target_os = "windows")]
const NAMES: &[&str] = &["libmpv-2.dll", "mpv-2.dll", "libmpv.dll"];
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const NAMES: &[&str] = &["libmpv.so.2", "libmpv.so"];

fn bind(library: *mut c_void) -> Option<Api> {
    macro_rules! find {
        ($name:literal) => {{
            let found = symbol(library, concat!($name, "\0"));
            if found.is_null() {
                return None;
            }
            unsafe { std::mem::transmute(found) }
        }};
    }
    Some(Api {
        create: find!("mpv_create"),
        initialize: find!("mpv_initialize"),
        terminate_destroy: find!("mpv_terminate_destroy"),
        set_option_string: find!("mpv_set_option_string"),
        set_property: find!("mpv_set_property"),
        get_property: find!("mpv_get_property"),
        command: find!("mpv_command"),
        render_create: find!("mpv_render_context_create"),
        render_set_update_callback: find!("mpv_render_context_set_update_callback"),
        render_render: find!("mpv_render_context_render"),
        render_free: find!("mpv_render_context_free"),
    })
}

pub struct Player {
    handle: Handle,
    render: Context,
    refused: Vec<String>,
    /// Handed to the update callback; freed only after the render context that holds it.
    ticket: *mut u64,
}

impl Player {
    /// Needs the canvas GL context current; options go before `initialize`, after which `vo` is read-only.
    pub fn new(
        get_proc_address: extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
        proc_ctx: *mut c_void,
        wake: Wake,
        instance: u64,
    ) -> Result<Player, String> {
        let api = api().ok_or("no libmpv on this machine")?;
        // mpv requires LC_NUMERIC=C; a comma-decimal desktop locale breaks it.
        plain_numbers();
        let handle = (api.create)();
        if handle.is_null() {
            return Err("mpv_create answered nothing".to_string());
        }
        let mut player = Player {
            handle,
            render: std::ptr::null_mut(),
            refused: Vec::new(),
            ticket: Box::into_raw(Box::new(instance)),
        };
        for (name, value) in OPTIONS {
            if !player.option(name, value) {
                player.refused.push((*name).to_string());
            }
        }
        if (api.initialize)(handle) < 0 {
            return Err("mpv_initialize refused".to_string());
        }

        let mut kind = *b"opengl\0";
        let mut init = GlInit {
            get_proc_address,
            ctx: proc_ctx,
        };
        let mut params = [
            Param {
                kind: PARAM_API_TYPE,
                data: kind.as_mut_ptr() as *mut c_void,
            },
            Param {
                kind: PARAM_OPENGL_INIT_PARAMS,
                data: &mut init as *mut GlInit as *mut c_void,
            },
            Param {
                kind: PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        let mut render: Context = std::ptr::null_mut();
        if (api.render_create)(&mut render, handle, params.as_mut_ptr()) < 0 {
            return Err("mpv would not render on this GL context".to_string());
        }
        player.render = render;
        (api.render_set_update_callback)(render, wake, player.ticket as *mut c_void);
        Ok(player)
    }

    fn option(&self, name: &str, value: &str) -> bool {
        let (Ok(api), Ok(name), Ok(value)) = (api().ok_or(()), CString::new(name), CString::new(value))
        else {
            return false;
        };
        (api.set_option_string)(self.handle, name.as_ptr(), value.as_ptr()) >= 0
    }

    pub fn refused(&self) -> &[String] {
        &self.refused
    }

    pub fn play(&self, path: &str) -> Result<(), String> {
        let api = api().ok_or("no libmpv on this machine")?;
        let (Ok(what), Ok(path)) = (CString::new("loadfile"), CString::new(path)) else {
            return Err("the path cannot be spelled for mpv".to_string());
        };
        let mut args = [what.as_ptr(), path.as_ptr(), std::ptr::null()];
        if (api.command)(self.handle, args.as_mut_ptr()) < 0 {
            return Err("mpv would not open the file".to_string());
        }
        Ok(())
    }

    pub fn draw(&self, fbo: i32, width: i32, height: i32) {
        let Some(api) = api() else {
            return;
        };
        if self.render.is_null() {
            return;
        }
        let mut target = Fbo {
            fbo,
            width,
            height,
            internal_format: 0,
        };
        // GL's origin is bottom-left; mpv renders top-down.
        let mut flip: c_int = 1;
        let mut params = [
            Param {
                kind: PARAM_OPENGL_FBO,
                data: &mut target as *mut Fbo as *mut c_void,
            },
            Param {
                kind: PARAM_FLIP_Y,
                data: &mut flip as *mut c_int as *mut c_void,
            },
            Param {
                kind: PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        (api.render_render)(self.render, params.as_mut_ptr());
    }

    pub fn flag(&self, name: &str) -> Option<bool> {
        let mut held: c_int = 0;
        self.read(name, FORMAT_FLAG, &mut held as *mut c_int as *mut c_void)
            .then_some(held != 0)
    }

    pub fn set_flag(&self, name: &str, yes: bool) {
        let mut held: c_int = c_int::from(yes);
        self.write(name, FORMAT_FLAG, &mut held as *mut c_int as *mut c_void);
    }

    pub fn number(&self, name: &str) -> Option<f64> {
        let mut held: c_double = 0.0;
        self.read(name, FORMAT_DOUBLE, &mut held as *mut c_double as *mut c_void)
            .then_some(held)
    }

    pub fn set_number(&self, name: &str, value: f64) {
        let mut held: c_double = value;
        self.write(name, FORMAT_DOUBLE, &mut held as *mut c_double as *mut c_void);
    }

    fn read(&self, name: &str, format: c_int, into: *mut c_void) -> bool {
        let (Some(api), Ok(name)) = (api(), CString::new(name)) else {
            return false;
        };
        (api.get_property)(self.handle, name.as_ptr(), format, into) >= 0
    }

    fn write(&self, name: &str, format: c_int, from: *mut c_void) {
        let (Some(api), Ok(name)) = (api(), CString::new(name)) else {
            return;
        };
        (api.set_property)(self.handle, name.as_ptr(), format, from);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let Some(api) = api() else {
            return;
        };
        // Render context first, while its GL context is current: it is what calls back with the ticket.
        if !self.render.is_null() {
            (api.render_free)(self.render);
        }
        if !self.ticket.is_null() {
            drop(unsafe { Box::from_raw(self.ticket) });
        }
        if !self.handle.is_null() {
            (api.terminate_destroy)(self.handle);
        }
    }
}

const OPTIONS: &[(&str, &str)] = &[
    ("vo", "libmpv"),
    ("config", "no"),
    ("terminal", "no"),
    // No `osc`: the LGPL libmpv is built without the player and rejects it.
    ("input-default-bindings", "no"),
    ("input-vo-keyboard", "no"),
    ("hwdec", "auto-safe"),
    ("keep-open", "yes"),
    ("volume", "80"),
];

/// Must match `volume` in OPTIONS.
pub const STARTING_VOLUME: f64 = 80.0;

#[cfg(any(target_os = "macos", target_os = "windows"))]
const LC_NUMERIC: c_int = 4;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const LC_NUMERIC: c_int = 1;

extern "C" {
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}

fn plain_numbers() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| unsafe {
        setlocale(LC_NUMERIC, b"C\0".as_ptr() as *const c_char);
    });
}

#[cfg(not(target_os = "windows"))]
mod system {
    use super::*;
    use std::ffi::CStr;

    const RTLD_NOW: c_int = 2;

    #[repr(C)]
    struct DlInfo {
        file_name: *const c_char,
        file_base: *mut c_void,
        symbol_name: *const c_char,
        symbol_address: *mut c_void,
    }

    extern "C" {
        fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dladdr(address: *const c_void, info: *mut DlInfo) -> c_int;
    }

    pub fn open(at: &str) -> Option<*mut c_void> {
        let named = CString::new(at).ok()?;
        let library = unsafe { dlopen(named.as_ptr(), RTLD_NOW) };
        (!library.is_null()).then_some(library)
    }

    /// Global, so a library loaded later that names this one by soname is linked to it rather than searching.
    #[cfg(target_os = "linux")]
    pub fn open_global(at: &str) -> bool {
        const RTLD_GLOBAL: c_int = 0x100;
        let Ok(named) = CString::new(at) else {
            return false;
        };
        !unsafe { dlopen(named.as_ptr(), RTLD_NOW | RTLD_GLOBAL) }.is_null()
    }

    pub fn symbol(library: *mut c_void, name: &str) -> *mut c_void {
        unsafe { dlsym(library, name.as_ptr() as *const c_char) }
    }

    pub fn own_folder() -> Option<PathBuf> {
        let mut told = DlInfo {
            file_name: std::ptr::null(),
            file_base: std::ptr::null_mut(),
            symbol_name: std::ptr::null(),
            symbol_address: std::ptr::null_mut(),
        };
        let here = own_folder as *const () as *const c_void;
        if unsafe { dladdr(here, &mut told) } == 0 || told.file_name.is_null() {
            return None;
        }
        let named = unsafe { CStr::from_ptr(told.file_name) }
            .to_string_lossy()
            .into_owned();
        PathBuf::from(named).parent().map(PathBuf::from)
    }
}

#[cfg(target_os = "windows")]
mod system {
    use super::*;

    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;

    /// Looks for the library's own dependencies in its folder rather than beside the application.
    const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;

    extern "system" {
        fn LoadLibraryA(name: *const c_char) -> *mut c_void;
        fn LoadLibraryExA(name: *const c_char, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(library: *mut c_void, name: *const c_char) -> *mut c_void;
        fn GetModuleHandleExA(flags: u32, name: *const c_char, held: *mut *mut c_void) -> c_int;
        fn GetModuleFileNameA(library: *mut c_void, into: *mut u8, room: u32) -> u32;
    }

    pub fn open(at: &str) -> Option<*mut c_void> {
        let named = CString::new(at).ok()?;
        // The flag is defined for full paths only; a bare name keeps the ordinary search.
        let library = if std::path::Path::new(at).is_absolute() {
            unsafe {
                LoadLibraryExA(
                    named.as_ptr(),
                    std::ptr::null_mut(),
                    LOAD_WITH_ALTERED_SEARCH_PATH,
                )
            }
        } else {
            unsafe { LoadLibraryA(named.as_ptr()) }
        };
        (!library.is_null()).then_some(library)
    }

    pub fn symbol(library: *mut c_void, name: &str) -> *mut c_void {
        unsafe { GetProcAddress(library, name.as_ptr() as *const c_char) }
    }

    pub fn own_folder() -> Option<PathBuf> {
        let mut held: *mut c_void = std::ptr::null_mut();
        let here = own_folder as *const () as *const c_char;
        if unsafe { GetModuleHandleExA(FROM_ADDRESS | UNCHANGED_REFCOUNT, here, &mut held) } == 0 {
            return None;
        }
        let mut room = [0u8; 4096];
        let written = unsafe { GetModuleFileNameA(held, room.as_mut_ptr(), room.len() as u32) };
        if written == 0 || written as usize >= room.len() {
            return None;
        }
        let named = String::from_utf8_lossy(&room[..written as usize]).into_owned();
        PathBuf::from(named).parent().map(PathBuf::from)
    }
}

#[cfg(target_os = "linux")]
use system::open_global;
use system::{open, own_folder, symbol};
