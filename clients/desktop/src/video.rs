//! Playing videos with mpv (libmpv), loaded when first needed: `libmpv.so.2` on Linux
//! (Fedora: `mpv-libs`), `libmpv-2.dll` / `mpv-2.dll` next to the app on Windows,
//! `libmpv.2.dylib` on macOS. Without it the app runs all the same; videos say what to
//! install.
//!
//! mpv decodes (in hardware where it can), plays the sound itself, and renders each frame in
//! software ("sw" render API) at the size it's shown, into a buffer that becomes an egui
//! texture: no OpenGL needed from egui (it draws with wgpu), and the cost follows the
//! size on screen, not the video's.
//!
//! Only the few calls used are declared here (see mpv's client.h and render.h).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::OnceLock;

use eframe::egui;

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct Event {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

// render.h
const RENDER_PARAM_INVALID: c_int = 0;
const RENDER_PARAM_API_TYPE: c_int = 1;
const RENDER_PARAM_SW_SIZE: c_int = 17;
const RENDER_PARAM_SW_FORMAT: c_int = 18;
const RENDER_PARAM_SW_STRIDE: c_int = 19;
const RENDER_PARAM_SW_POINTER: c_int = 20;
const RENDER_UPDATE_FRAME: u64 = 1;
// client.h
const FORMAT_FLAG: c_int = 3;
const FORMAT_DOUBLE: c_int = 5;
const EVENT_NONE: c_int = 0;

type Handle = c_void;

struct Lib {
    _lib: libloading::Library,
    create: unsafe extern "C" fn() -> *mut Handle,
    initialize: unsafe extern "C" fn(*mut Handle) -> c_int,
    set_option_string: unsafe extern "C" fn(*mut Handle, *const c_char, *const c_char) -> c_int,
    set_property_string: unsafe extern "C" fn(*mut Handle, *const c_char, *const c_char) -> c_int,
    get_property: unsafe extern "C" fn(*mut Handle, *const c_char, c_int, *mut c_void) -> c_int,
    command: unsafe extern "C" fn(*mut Handle, *mut *const c_char) -> c_int,
    wait_event: unsafe extern "C" fn(*mut Handle, f64) -> *mut Event,
    terminate_destroy: unsafe extern "C" fn(*mut Handle),
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    render_create: unsafe extern "C" fn(*mut *mut c_void, *mut Handle, *mut RenderParam) -> c_int,
    render_render: unsafe extern "C" fn(*mut c_void, *mut RenderParam) -> c_int,
    render_update: unsafe extern "C" fn(*mut c_void) -> u64,
    render_set_update_callback: unsafe extern "C" fn(*mut c_void, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    render_free: unsafe extern "C" fn(*mut c_void),
}

/// The library's names, most likely first.
const NAMES: &[&str] = if cfg!(target_os = "windows") {
    &["libmpv-2.dll", "mpv-2.dll", "mpv-1.dll"]
} else if cfg!(target_os = "macos") {
    &["libmpv.2.dylib", "libmpv.dylib"]
} else {
    &["libmpv.so.2", "libmpv.so", "libmpv.so.1"]
};

/// What to install, said when mpv isn't there.
pub fn missing_text() -> &'static str {
    if cfg!(target_os = "windows") {
        "Videos need mpv's library: put libmpv-2.dll next to the app."
    } else if cfg!(target_os = "macos") {
        "Videos need mpv's library (brew install mpv)."
    } else {
        "Videos need mpv's library: install it (Fedora: sudo dnf install mpv-libs; Debian/Ubuntu: libmpv2)."
    }
}

fn lib() -> Result<&'static Lib, String> {
    static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();
    LIB.get_or_init(load).as_ref().map_err(|e| e.clone())
}

fn load() -> Result<Lib, String> {
    let mut last = String::new();
    for name in NAMES {
        // Safety: loading a system library by name; its initialisers are mpv's own.
        let lib = match unsafe { libloading::Library::new(*name) } {
            Ok(lib) => lib,
            Err(err) => {
                last = err.to_string();
                continue;
            }
        };
        macro_rules! sym {
            ($name:literal) => {
                // Safety: the declared type is the one in mpv's headers.
                *unsafe { lib.get(concat!($name, "\0").as_bytes()) }.map_err(|e| format!("{}: {e}", $name))?
            };
        }
        return Ok(Lib {
            create: sym!("mpv_create"),
            initialize: sym!("mpv_initialize"),
            set_option_string: sym!("mpv_set_option_string"),
            set_property_string: sym!("mpv_set_property_string"),
            get_property: sym!("mpv_get_property"),
            command: sym!("mpv_command"),
            wait_event: sym!("mpv_wait_event"),
            terminate_destroy: sym!("mpv_terminate_destroy"),
            error_string: sym!("mpv_error_string"),
            render_create: sym!("mpv_render_context_create"),
            render_render: sym!("mpv_render_context_render"),
            render_update: sym!("mpv_render_context_update"),
            render_set_update_callback: sym!("mpv_render_context_set_update_callback"),
            render_free: sym!("mpv_render_context_free"),
            _lib: lib,
        });
    }
    Err(format!("{} ({last})", missing_text()))
}

/// Whether mpv can be used here (loads it the first time).
#[cfg(test)]
pub fn available() -> Result<(), String> {
    lib().map(|_| ())
}

/// A video playing: its mpv, and the frames it renders.
pub struct Player {
    lib: &'static Lib,
    handle: *mut Handle,
    render: *mut c_void,
    /// Asked to redraw when a frame is ready (from mpv's thread).
    _repaint: Box<egui::Context>,
    buffer: Vec<u8>,
}

unsafe extern "C" fn frame_ready(data: *mut c_void) {
    // Safety: `data` is the `Box<egui::Context>` the player keeps alive until the render
    // context is freed.
    let ctx = unsafe { &*(data as *const egui::Context) };
    ctx.request_repaint();
}

impl Player {
    /// Starts playing `url` (sent `header`, the login), at `volume` (0–100).
    pub fn open(ctx: &egui::Context, url: &str, header: &str, volume: f64) -> Result<Player, String> {
        let lib = lib()?;
        // Safety: every call below follows client.h/render.h; the handle and render
        // context live until `drop`.
        unsafe {
            let handle = (lib.create)();
            if handle.is_null() {
                return Err("mpv couldn't start".into());
            }
            let set = |name: &str, value: &str| {
                let (n, v) = (CString::new(name).unwrap(), CString::new(value).unwrap_or_default());
                (lib.set_option_string)(handle, n.as_ptr(), v.as_ptr())
            };
            set("vo", "libmpv");
            set("hwdec", "auto-copy-safe");
            set("keep-open", "yes");
            set("terminal", "no");
            set("input-default-bindings", "no");
            set("osc", "no");
            set("audio-client-name", "Acetate");
            set("http-header-fields", header);
            set("volume", &format!("{volume:.0}"));
            let code = (lib.initialize)(handle);
            if code < 0 {
                (lib.terminate_destroy)(handle);
                return Err(format!("mpv couldn't start: {}", error(lib, code)));
            }
            let api = CString::new("sw").unwrap();
            let mut params = [
                RenderParam { kind: RENDER_PARAM_API_TYPE, data: api.as_ptr() as *mut c_void },
                RenderParam { kind: RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
            ];
            let mut render = std::ptr::null_mut();
            let code = (lib.render_create)(&mut render, handle, params.as_mut_ptr());
            if code < 0 {
                (lib.terminate_destroy)(handle);
                return Err(format!("mpv couldn't render: {}", error(lib, code)));
            }
            let repaint = Box::new(ctx.clone());
            (lib.render_set_update_callback)(render, Some(frame_ready), &*repaint as *const egui::Context as *mut c_void);
            let player = Player { lib, handle, render, _repaint: repaint, buffer: Vec::new() };
            player.command(&["loadfile", url])?;
            Ok(player)
        }
    }

    /// Runs an mpv command ("seek", "5", "relative"…).
    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap_or_default()).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        // Safety: a NULL-terminated array of C strings that outlive the call.
        let code = unsafe { (self.lib.command)(self.handle, ptrs.as_mut_ptr()) };
        if code < 0 { Err(error(self.lib, code)) } else { Ok(()) }
    }

    pub fn set(&self, name: &str, value: &str) {
        let (n, v) = (CString::new(name).unwrap(), CString::new(value).unwrap_or_default());
        // Safety: C strings that outlive the call.
        unsafe { (self.lib.set_property_string)(self.handle, n.as_ptr(), v.as_ptr()) };
    }

    fn double(&self, name: &str) -> Option<f64> {
        let n = CString::new(name).unwrap();
        let mut out = 0f64;
        // Safety: MPV_FORMAT_DOUBLE writes a double.
        let code = unsafe { (self.lib.get_property)(self.handle, n.as_ptr(), FORMAT_DOUBLE, &mut out as *mut f64 as *mut c_void) };
        (code >= 0).then_some(out)
    }

    fn flag(&self, name: &str) -> Option<bool> {
        let n = CString::new(name).unwrap();
        let mut out: c_int = 0;
        // Safety: MPV_FORMAT_FLAG writes an int.
        let code = unsafe { (self.lib.get_property)(self.handle, n.as_ptr(), FORMAT_FLAG, &mut out as *mut c_int as *mut c_void) };
        (code >= 0).then_some(out != 0)
    }

    /// Where it is, and how long it is (seconds; None before it's loaded).
    pub fn position(&self) -> Option<f64> {
        self.double("time-pos")
    }

    pub fn duration(&self) -> Option<f64> {
        self.double("duration")
    }

    pub fn paused(&self) -> bool {
        self.flag("pause").unwrap_or(false)
    }

    /// The video's own size (pixels), once known.
    #[cfg(test)]
    pub fn video_size(&self) -> Option<(f64, f64)> {
        Some((self.double("width")?, self.double("height")?))
    }

    /// mpv's events, taken (it keeps them until they are). Call once per frame.
    pub fn drain_events(&self) {
        // Safety: the returned event is only read before the next call.
        unsafe {
            loop {
                let event = (self.lib.wait_event)(self.handle, 0.0);
                if event.is_null() || (*event).event_id == EVENT_NONE {
                    break;
                }
            }
        }
    }

    /// A new frame, if there's one, rendered at `width`×`height` (the video fits inside,
    /// black around it). `again`: rendered even without a new one (the size changed while
    /// paused).
    pub fn frame(&mut self, width: usize, height: usize, again: bool) -> Option<egui::ColorImage> {
        if width == 0 || height == 0 {
            return None;
        }
        // Safety: the render context is valid until `drop`.
        let flags = unsafe { (self.lib.render_update)(self.render) };
        if flags & RENDER_UPDATE_FRAME == 0 && !again {
            return None;
        }
        let stride = width * 4;
        self.buffer.resize(stride * height, 0);
        let mut size = [width as c_int, height as c_int];
        let format = CString::new("rgb0").unwrap();
        let mut stride_param = stride;
        let mut params = [
            RenderParam { kind: RENDER_PARAM_SW_SIZE, data: size.as_mut_ptr() as *mut c_void },
            RenderParam { kind: RENDER_PARAM_SW_FORMAT, data: format.as_ptr() as *mut c_void },
            RenderParam { kind: RENDER_PARAM_SW_STRIDE, data: &mut stride_param as *mut usize as *mut c_void },
            RenderParam { kind: RENDER_PARAM_SW_POINTER, data: self.buffer.as_mut_ptr() as *mut c_void },
            RenderParam { kind: RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        // Safety: the buffer holds `stride * height` bytes, as the params say.
        let code = unsafe { (self.lib.render_render)(self.render, params.as_mut_ptr()) };
        if code < 0 {
            return None;
        }
        // rgb0 → RGBA (the fourth byte is padding).
        for px in self.buffer.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Some(egui::ColorImage::from_rgba_unmultiplied([width, height], &self.buffer))
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Safety: the render context goes first (it belongs to the handle), then mpv.
        unsafe {
            (self.lib.render_free)(self.render);
            (self.lib.terminate_destroy)(self.handle);
        }
    }
}

fn error(lib: &Lib, code: c_int) -> String {
    // Safety: mpv returns a static string for any error code.
    unsafe { CStr::from_ptr((lib.error_string)(code)) }.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With libmpv installed: a video made on the spot (ffmpeg) plays, and frames come out
    /// at the size asked for. `cargo test video -- --ignored`.
    #[test]
    #[ignore]
    fn a_video_plays_and_renders_frames() {
        if available().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("acetate-video-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.mp4");
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=3", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            return;
        }
        let ctx = egui::Context::default();
        let mut player = Player::open(&ctx, path.to_str().unwrap(), "X-Test: 1", 0.0).unwrap();
        let mut got = None;
        for _ in 0..200 {
            player.drain_events();
            if let Some(frame) = player.frame(160, 120, false) {
                got = Some(frame);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let frame = got.expect("a frame");
        assert_eq!(frame.size, [160, 120]);
        assert!(frame.pixels.iter().any(|p| p.r() > 100), "not all black");
        assert_eq!(player.video_size(), Some((320.0, 240.0)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A video streamed from a running server, as the app watches one (the login in a
    /// header, seeking by ranges): `ACETATE_TEST_VIDEO_URL=http://…/api/extra-files/<id>/stream
    /// ACETATE_TEST_TOKEN=… cargo test streamed -- --ignored`.
    #[test]
    #[ignore]
    fn a_video_streamed_from_the_server_plays_and_seeks() {
        let (Ok(url), Ok(token)) = (std::env::var("ACETATE_TEST_VIDEO_URL"), std::env::var("ACETATE_TEST_TOKEN")) else { return };
        let ctx = egui::Context::default();
        let mut player = Player::open(&ctx, &url, &format!("Authorization: Bearer {token}"), 0.0).unwrap();
        let mut frames = 0;
        for _ in 0..300 {
            player.drain_events();
            if player.frame(64, 36, false).is_some() {
                frames += 1;
            }
            if frames >= 3 && player.duration().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(frames >= 3, "frames came");
        let duration = player.duration().unwrap();
        player.command(&["seek", &format!("{:.1}", duration / 2.0), "absolute"]).unwrap();
        let mut at = 0.0;
        for _ in 0..200 {
            player.drain_events();
            at = player.position().unwrap_or(0.0);
            if at >= duration / 2.0 - 0.5 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(at >= duration / 2.0 - 0.5, "seeked to the middle ({at} of {duration})");
    }
}
