#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod mpv;

use ic_plugin_api::{
    check_host, needs_canvas, HostCheck, IcBytes, IcCanvas, IcFrame, IcFsSource, IcHost,
    IcViewVTable, IcViewerVTable, IC_ABI_VERSION, IC_CANVAS_GL, IC_ERR_HOST_TOO_OLD,
    IC_ERR_HOST_UNKNOWN, IC_ERR_INIT_FAILED, IC_ERR_IO, IC_ERR_NOT_THIS_HOST, IC_HOST_GTK, IC_OK,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../version.rs"));

ic_plugin_api::declare_about!(
    "ic-video-view",
    "Video",
    plugin_version!(),
    "Plays video, with the codecs it needs"
);

pub const ID: &str = "video-view";
pub const EXTENSIONS: &str =
    ".mp4,.mkv,.avi,.mov,.webm,.m4v,.wmv,.flv,.ts,.m2ts,.mts,.mpg,.mpeg,.3gp,.ogv";

// TODO: no node or accel emits `back`/`on`, so this ten-second step is unreachable.
const STEP: f64 = 10.0;

static HOST: AtomicUsize = AtomicUsize::new(0);
static DRAWS: AtomicBool = AtomicBool::new(false);

fn host() -> *const IcHost {
    HOST.load(Ordering::Relaxed) as *const IcHost
}

struct Showing {
    named: String,
    /// A host-made temporary copy when the file is inside an archive or on a server.
    local: Option<String>,
    player: Option<mpv::Player>,
    trouble: Option<String>,
    /// Reported instead of time-pos for a moment after a seek, or a refresh would pull the handle back.
    dragged_to: Option<(f64, std::time::Instant)>,
}

thread_local! {
    static SHOWING: RefCell<BTreeMap<u64, Showing>> = const { RefCell::new(BTreeMap::new()) };
    static DOCUMENT: RefCell<String> = const { RefCell::new(String::new()) };
    static ANSWER: RefCell<String> = const { RefCell::new(String::new()) };
}

extern "C" fn viewer_open(
    instance: u64,
    source: IcFsSource,
    path: *const c_char,
    _user_data: *mut c_void,
) -> c_int {
    if path.is_null() {
        return IC_ERR_INIT_FAILED;
    }
    let named = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .trim_start_matches('/')
        .to_string();
    // Not on the web host: fs_local_path would copy a remote file down for nobody.
    let local = DRAWS
        .load(Ordering::Relaxed)
        .then(|| local_path(source, &named))
        .flatten();
    let trouble = (DRAWS.load(Ordering::Relaxed) && local.is_none())
        .then(|| "This file cannot be opened by the player.".to_string());
    SHOWING.with(|held| {
        held.borrow_mut().insert(
            instance,
            Showing {
                named,
                local,
                player: None,
                trouble,
                dragged_to: None,
            },
        )
    });
    IC_OK
}

fn local_path(source: IcFsSource, named: &str) -> Option<String> {
    let host = host();
    if host.is_null() {
        return None;
    }
    let asked = CString::new(named).ok()?;
    let answered = unsafe { ((*host).fs_local_path)(source, asked.as_ptr()) };
    (!answered.is_null()).then(|| {
        unsafe { CStr::from_ptr(answered) }
            .to_string_lossy()
            .into_owned()
    })
}

extern "C" fn viewer_closed(instance: u64, _user_data: *mut c_void) {
    // The player is dropped in canvas_gone, while its GL context is still current.
    SHOWING.with(|held| held.borrow_mut().remove(&instance));
}

extern "C" fn canvas_ready(instance: u64, canvas: *const IcCanvas, _user_data: *mut c_void) -> c_int {
    if canvas.is_null() {
        return IC_ERR_INIT_FAILED;
    }
    let canvas = unsafe { *canvas };
    if canvas.api != IC_CANVAS_GL {
        return IC_ERR_NOT_THIS_HOST;
    }
    let started = mpv::Player::new(
        canvas.get_proc_address,
        canvas.proc_ctx,
        frame_is_ready,
        instance,
    )
    .and_then(|player| {
        let path = SHOWING.with(|held| {
            held.borrow()
                .get(&instance)
                .and_then(|showing| showing.local.clone())
        });
        let path = path.ok_or_else(|| "there is no file to play".to_string())?;
        player.play(&path)?;
        Ok(player)
    });
    SHOWING.with(|held| {
        let mut held = held.borrow_mut();
        let Some(showing) = held.get_mut(&instance) else {
            return IC_ERR_IO;
        };
        match started {
            Ok(player) => {
                if !player.refused().is_empty() {
                    warn(&format!(
                        "video: libmpv would not take {}",
                        player.refused().join(", ")
                    ));
                }
                showing.player = Some(player);
                showing.trouble = None;
                IC_OK
            }
            Err(wrong) => {
                warn(&format!("video: {wrong} (libmpv at {})", mpv::whence()));
                showing.trouble = Some(wrong);
                IC_ERR_IO
            }
        }
    })
}

extern "C" fn canvas_draw(instance: u64, frame: *const IcFrame, _user_data: *mut c_void) {
    if frame.is_null() {
        return;
    }
    let frame = unsafe { *frame };
    SHOWING.with(|held| {
        if let Some(player) = held
            .borrow()
            .get(&instance)
            .and_then(|showing| showing.player.as_ref())
        {
            player.draw(frame.fbo, frame.width, frame.height);
        }
    });
}

extern "C" fn canvas_gone(instance: u64, _user_data: *mut c_void) {
    SHOWING.with(|held| {
        if let Some(showing) = held.borrow_mut().get_mut(&instance) {
            showing.player = None;
        }
    });
}

/// Runs on an mpv thread: canvas_invalidate is the only host call allowed here.
extern "C" fn frame_is_ready(ticket: *mut c_void) {
    if ticket.is_null() {
        return;
    }
    let instance = unsafe { *(ticket as *mut u64) };
    let host = host();
    if !host.is_null() {
        unsafe { ((*host).canvas_invalidate)(instance) };
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Where {
    pub at: f64,
    pub of: f64,
    pub paused: bool,
    pub volume: f64,
}

/// A fixed slider range in thousandths: putting the duration in the form would change its shape.
pub const SPAN: f64 = 1000.0;

pub const VOLUME_WIDTH: u32 = 200;

pub const MARK: u32 = 28;

const PLAY: &[u8] = include_bytes!("../assets/play.svg");
const PAUSE: &[u8] = include_bytes!("../assets/pause.svg");

pub fn clock(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".to_string();
    }
    let whole = seconds as u64;
    let (hours, minutes, seconds) = (whole / 3600, (whole / 60) % 60, whole % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

pub fn along(showing: Where) -> f64 {
    if showing.of > 0.0 {
        (showing.at / showing.of * SPAN).clamp(0.0, SPAN)
    } else {
        0.0
    }
}

pub fn seconds_at(part: f64, of: f64) -> f64 {
    (part / SPAN * of).clamp(0.0, of.max(0.0))
}

fn while_playing() -> serde_json::Value {
    serde_json::json!({ "not": { "truthy": "data.trouble" } })
}

fn play_button() -> serde_json::Value {
    serde_json::json!({
        "t": "button",
        "id": "play",
        "role": "flat",
        "height": MARK,
        "icon": either(
            "data.paused",
            &format!("asset:{ID}/play"),
            &format!("asset:{ID}/pause")
        ),
        "tooltip": either("data.paused", "Play", "Pause"),
        "accel": "space",
        "visible": while_playing(),
        "intent": { "do": "emit", "node": "play" }
    })
}

fn either(asking: &str, yes: &str, no: &str) -> serde_json::Value {
    serde_json::json!({
        "cases": [ { "when": { "truthy": asking }, "then": { "literal": yes } } ],
        "else": { "literal": no }
    })
}

/// The form must never change shape: a rebuild unparents the canvas, which in GTK is a new GL context and the film restarting.
pub fn drawn_document(showing: Where, trouble: bool) -> String {
    serde_json::json!({
        "schema": 1,
        "data": {
            "paused": showing.paused,
            "trouble": trouble,
            "at": along(showing),
            "volume": showing.volume,
            // A clock is text that moves, and only a table's cells can do that.
            "clock": [ { "t": format!("{} / {}", clock(showing.at), clock(showing.of)) } ]
        },
        "fields": [],
        "form": {
            "t": "view",
            "surface": "window",
            "spacing": 0,
            "padding": 0,
            "refresh_ms": 500,
            "children": [
                { "t": "canvas", "id": "screen", "weight": 1, "visible": while_playing() },
                {
                    "t": "text", "id": "trouble", "role": "dim", "weight": 1, "wrap": true,
                    "visible": { "truthy": "data.trouble" },
                    "text": { "literal": "This file could not be played." }
                },
                {
                    "t": "row", "id": "controls", "spacing": 8, "padding": 8,
                    "visible": {
                        "all": [
                            while_playing(),
                            { "not": { "truthy": "view.pointer_idle" } }
                        ]
                    },
                    "children": [
                        play_button(),
                        {
                            "t": "slider", "id": "seek", "weight": 1,
                            "value_key": "at", "min": 0, "max": SPAN, "step": 1,
                            "emit": "change", "debounce_ms": 120,
                            "visible": while_playing(),
                            "tooltip": { "literal": "Where in the film" }
                        },
                        {
                            "t": "table", "id": "clock", "rows_key": "clock",
                            "columns": [ { "key": "t" } ],
                            "visible": while_playing()
                        },
                        {
                            "t": "slider", "id": "volume", "width": VOLUME_WIDTH,
                            "value_key": "volume", "min": 0, "max": 100, "step": 1,
                            "emit": "change", "debounce_ms": 80,
                            "visible": while_playing(),
                            "tooltip": { "literal": "Volume" }
                        }
                    ]
                }
            ]
        }
    })
    .to_string()
}

pub fn played_document(named: &str) -> String {
    serde_json::json!({
        "schema": 1,
        "data": {},
        "fields": [],
        "form": {
            "t": "view",
            "surface": "window",
            "padding": 0,
            "children": [{
                "t": "media",
                "id": "screen",
                "media": "video",
                "src": format!("file:{named}"),
                "fit": "contain",
                "autoplay": true,
                "weight": 1
            }]
        }
    })
    .to_string()
}

fn asking_about(raw: *const u8, len: u64) -> u64 {
    if raw.is_null() || len == 0 {
        return 0;
    }
    let held = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(raw, len as usize) });
    serde_json::from_str::<serde_json::Value>(&held)
        .ok()
        .and_then(|held| held.get("instance").and_then(|at| at.as_u64()))
        .unwrap_or(0)
}

extern "C" fn viewer_describe(ctx: *const u8, len: u64, _user_data: *mut c_void) -> IcBytes {
    let asked = asking_about(ctx, len);
    let drawn = SHOWING.with(|held| {
        let held = held.borrow();
        let showing = held.get(&asked)?;
        if !DRAWS.load(Ordering::Relaxed) {
            return Some(played_document(&showing.named));
        }
        let at = match showing.player.as_ref() {
            Some(player) => Where {
                at: showing
                    .holding()
                    .or_else(|| player.number("time-pos"))
                    .unwrap_or(0.0),
                of: player.number("duration").unwrap_or(0.0),
                paused: player.flag("pause").unwrap_or(false),
                volume: player.number("volume").unwrap_or(mpv::STARTING_VOLUME),
            },
            None => Where::default(),
        };
        Some(drawn_document(at, showing.trouble.is_some()))
    });
    let Some(drawn) = drawn else {
        return IcBytes::EMPTY;
    };
    DOCUMENT.with(|held| {
        let mut held = held.borrow_mut();
        *held = drawn;
        IcBytes {
            data: held.as_ptr(),
            len: held.len() as u64,
        }
    })
}

/// Covers the refresh after the last drag step without making a slow seek wait.
const A_DRAG_LASTS: std::time::Duration = std::time::Duration::from_millis(700);

impl Showing {
    fn holding(&self) -> Option<f64> {
        let (at, since) = self.dragged_to?;
        (since.elapsed() < A_DRAG_LASTS).then_some(at)
    }
}

/// Answers whether to redescribe at once; the sliders and the clock catch up on the next refresh.
fn pressed(showing: &mut Showing, node: &str, value: Option<f64>) -> bool {
    let Some(player) = showing.player.as_ref() else {
        return false;
    };
    match node {
        "play" => {
            player.set_flag("pause", !player.flag("pause").unwrap_or(false));
            true
        }
        "back" | "on" => {
            let at = player.number("time-pos").unwrap_or(0.0);
            let of = player.number("duration").unwrap_or(0.0);
            let wanted = if node == "back" { at - STEP } else { at + STEP };
            let wanted = wanted.clamp(0.0, of.max(0.0));
            player.set_number("time-pos", wanted);
            showing.dragged_to = Some((wanted, std::time::Instant::now()));
            true
        }
        "seek" => {
            let Some(part) = value else {
                return false;
            };
            let wanted = seconds_at(part, player.number("duration").unwrap_or(0.0));
            player.set_number("time-pos", wanted);
            showing.dragged_to = Some((wanted, std::time::Instant::now()));
            false
        }
        "volume" => {
            let Some(wanted) = value else {
                return false;
            };
            player.set_number("volume", wanted.clamp(0.0, 100.0));
            false
        }
        _ => false,
    }
}

extern "C" fn viewer_event(raw: *const u8, len: u64, _user_data: *mut c_void) -> IcBytes {
    let asked = asking_about(raw, len);
    let event: serde_json::Value = if raw.is_null() || len == 0 {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(raw, len as usize) })
            .unwrap_or(serde_json::Value::Null)
    };
    let node = event
        .get("node")
        .and_then(|node| node.as_str())
        .unwrap_or("");
    let moved = event.get("value").and_then(|value| value.as_f64());
    let again = SHOWING.with(|held| {
        let mut held = held.borrow_mut();
        held.get_mut(&asked)
            .is_some_and(|showing| pressed(showing, node, moved))
    });
    answer_with(if again {
        r#"{"redescribe":true}"#
    } else {
        "{}"
    })
}

fn answer_with(text: &str) -> IcBytes {
    ANSWER.with(|held| {
        let mut held = held.borrow_mut();
        *held = text.to_string();
        IcBytes {
            data: held.as_ptr(),
            len: held.len() as u64,
        }
    })
}

fn warn(message: &str) {
    let host = host();
    let Ok(message) = CString::new(message) else {
        return;
    };
    if !host.is_null() {
        unsafe { ((*host).log_warn)(message.as_ptr()) };
    }
}

pub fn view_vtable() -> IcViewVTable {
    IcViewVTable {
        struct_size: std::mem::size_of::<IcViewVTable>() as u32,
        describe: viewer_describe,
        on_event: Some(viewer_event),
        closed: None,
    }
}

pub fn viewer_vtable(view: *const IcViewVTable, draws: bool) -> IcViewerVTable {
    IcViewerVTable {
        struct_size: std::mem::size_of::<IcViewerVTable>() as u32,
        view,
        open: viewer_open,
        closed: Some(viewer_closed),
        content: None,
        closing: None,
        canvas_ready: draws.then_some(canvas_ready),
        canvas_draw: draws.then_some(canvas_draw),
        canvas_gone: draws.then_some(canvas_gone),
    }
}

fn frontend(kind: *const c_char) -> String {
    if kind.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(kind) }.to_string_lossy().into_owned()
}

#[cfg_attr(feature = "export-abi", no_mangle)]
pub extern "C" fn ic_plugin_init(host: *const IcHost, kind: *const c_char) -> c_int {
    let kind = frontend(kind);
    let draws = kind == IC_HOST_GTK;
    if kind != IC_HOST_GTK && kind != ic_plugin_api::IC_HOST_WEB {
        return IC_ERR_NOT_THIS_HOST;
    }
    match check_host(host, IC_ABI_VERSION, needs_canvas()) {
        HostCheck::Ok => {}
        HostCheck::WrongMagic => return IC_ERR_HOST_UNKNOWN,
        HostCheck::TooOld { .. } | HostCheck::Truncated { .. } => return IC_ERR_HOST_TOO_OLD,
    }
    HOST.store(host as usize, Ordering::Relaxed);
    // Refuse to load rather than claim the extensions away from a viewer that can still play them.
    if draws && !mpv::found() {
        warn("video: no libmpv was found, so this plugin plays nothing");
        return IC_ERR_INIT_FAILED;
    }
    DRAWS.store(draws, Ordering::Relaxed);
    let (Ok(id), Ok(extensions)) = (CString::new(ID), CString::new(EXTENSIONS)) else {
        return IC_ERR_INIT_FAILED;
    };
    for (name, picture) in [("play", PLAY), ("pause", PAUSE)] {
        let (Ok(owner), Ok(name)) = (CString::new(ID), CString::new(name)) else {
            continue;
        };
        unsafe {
            ((*host).register_plugin_asset)(
                owner.as_ptr(),
                name.as_ptr(),
                picture.as_ptr(),
                picture.len() as u64,
            )
        };
    }
    let window = view_vtable();
    let viewer = viewer_vtable(&window, draws);
    unsafe {
        ((*host).register_viewer)(
            id.as_ptr(),
            extensions.as_ptr(),
            0,
            &viewer,
            std::ptr::null_mut(),
        )
    }
}

#[cfg_attr(feature = "export-abi", no_mangle)]
pub extern "C" fn ic_plugin_shutdown() {
    SHOWING.with(|held| held.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(document: &str) -> serde_json::Value {
        serde_json::from_str(document).expect("a document")
    }

    fn controls(drawn: &serde_json::Value) -> &Vec<serde_json::Value> {
        drawn["form"]["children"][2]["children"]
            .as_array()
            .expect("a row of controls")
    }

    #[test]
    fn the_shape_of_the_window_never_changes_while_the_film_does() {
        let start = parsed(&drawn_document(
            Where { at: 0.0, of: 300.0, paused: false, volume: 80.0 },
            false,
        ));
        let later = parsed(&drawn_document(
            Where { at: 41.0, of: 300.0, paused: true, volume: 35.0 },
            false,
        ));
        let broken = parsed(&drawn_document(Where::default(), true));

        assert_eq!(start["form"], later["form"], "a film playing must not rebuild the window");
        assert_eq!(start["form"], broken["form"], "nor must failing to play at all");
        assert_ne!(start["data"], later["data"], "only the data moves");

        assert_eq!(start["data"]["at"], 0.0);
        assert_eq!(later["data"]["at"], 41.0 / 300.0 * SPAN);
        assert_eq!(later["data"]["volume"], 35.0);
        assert_eq!(later["data"]["paused"], true);
        assert_eq!(later["data"]["clock"][0]["t"], "0:41 / 5:00");
        assert_eq!(broken["data"]["trouble"], true);
    }

    #[test]
    fn the_window_is_a_canvas_with_controls_under_it() {
        let drawn = parsed(&drawn_document(
            Where { at: 10.0, of: 100.0, paused: false, volume: 80.0 },
            false,
        ));
        assert_eq!(drawn["form"]["children"][0]["t"], "canvas");
        assert_eq!(drawn["form"]["children"][0]["id"], "screen");

        let controls = controls(&drawn);
        assert_eq!(controls[0]["id"], "play");
        assert_eq!(controls[0]["accel"], "space");
        assert_eq!(controls[0]["role"], "flat", "a mark, not a button with a frame");
        assert!(controls[0]["label"].is_null(), "it wears a picture, not a word");
        assert_eq!(
            controls[0]["icon"]["cases"][0]["when"]["truthy"], "data.paused",
            "the mark is chosen by the data, which is what keeps the shape still"
        );
        assert_eq!(controls[0]["icon"]["cases"][0]["then"]["literal"], format!("asset:{ID}/play"));
        assert_eq!(controls[0]["icon"]["else"]["literal"], format!("asset:{ID}/pause"));

        let seek = &controls[1];
        assert_eq!(seek["t"], "slider");
        assert_eq!(seek["value_key"], "at");
        assert_eq!(seek["max"], SPAN);
        assert_eq!(seek["emit"], "change", "it is dragged, not committed");
        assert_eq!(seek["weight"], 1, "the room going spare belongs to the film");

        assert_eq!(controls[2]["t"], "table");
        assert_eq!(controls[2]["rows_key"], "clock");

        let volume = &controls[3];
        assert_eq!(volume["t"], "slider");
        assert_eq!(volume["value_key"], "volume");
        assert_eq!(volume["max"], 100.0);
        assert_eq!(volume["width"], VOLUME_WIDTH, "and a fixed width is not the film's");
        assert!(volume["weight"].is_null(), "so it must not expand");
    }

    #[test]
    fn the_two_marks_are_the_same_size_and_say_so() {
        let size_of = |picture: &[u8]| {
            let text = String::from_utf8_lossy(picture).into_owned();
            let field = |name: &str| {
                text.split_once(&format!("{name}=\""))
                    .and_then(|(_, rest)| rest.split_once('"'))
                    .map(|(value, _)| value.to_string())
            };
            (
                field("viewBox").expect("a viewBox"),
                field("width").expect("a width"),
                field("height").expect("a height"),
            )
        };
        assert_eq!(size_of(PLAY), size_of(PAUSE), "one size for both marks");
        let (box_of, width, height) = size_of(PLAY);
        assert_eq!(box_of, "0 0 80 80");
        assert_eq!(width, "80px", "said outright, not left to the drawing");
        assert_eq!(height, "80px");
    }

    #[test]
    fn the_clock_reads_as_a_person_would_say_it() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(9.7), "0:09", "what has passed, not what is nearest");
        assert_eq!(clock(247.0), "4:07");
        assert_eq!(clock(3800.0), "1:03:20", "an hour brings a third number");
        assert_eq!(clock(f64::NAN), "0:00", "a film that has not said yet");
        assert_eq!(clock(-1.0), "0:00");
    }

    #[test]
    fn the_slider_and_the_film_agree_both_ways() {
        let of = 637.0;
        for at in [0.0, 1.0, 318.5, 636.9, 637.0] {
            let there = along(Where { at, of, ..Where::default() });
            assert!((seconds_at(there, of) - at).abs() < 1.0, "{at}s came back as something else");
        }
        assert_eq!(along(Where { at: 5.0, of: 0.0, ..Where::default() }), 0.0,
                   "a film with no length sits at the start");
        assert_eq!(seconds_at(SPAN * 2.0, 100.0), 100.0, "and never past the end");
        assert_eq!(seconds_at(-5.0, 100.0), 0.0);
    }

    #[test]
    fn a_slider_under_the_hand_is_left_where_it_was_put() {
        let mut showing = Showing {
            named: "holiday.mp4".to_string(),
            local: None,
            player: None,
            trouble: None,
            dragged_to: None,
        };
        assert_eq!(showing.holding(), None, "nothing is held to begin with");

        showing.dragged_to = Some((120.0, std::time::Instant::now()));
        assert_eq!(showing.holding(), Some(120.0));

        showing.dragged_to = Some((120.0, std::time::Instant::now() - A_DRAG_LASTS));
        assert_eq!(showing.holding(), None, "and the film has it back afterwards");
    }

    #[test]
    fn what_went_wrong_takes_the_pictures_place() {
        let drawn = parsed(&drawn_document(Where::default(), true));
        let children = drawn["form"]["children"].as_array().expect("a window");
        assert_eq!(children[1]["id"], "trouble");
        assert_eq!(children[1]["visible"]["truthy"], "data.trouble");
        assert_eq!(children[0]["visible"]["not"]["truthy"], "data.trouble");
        assert_eq!(
            children[2]["visible"]["all"][0]["not"]["truthy"], "data.trouble",
            "the controls go when there is nothing playing"
        );
        assert_eq!(
            children[2]["visible"]["all"][1]["not"]["truthy"], "view.pointer_idle",
            "and again when the picture fills the screen and nobody stirs"
        );
    }

    #[test]
    fn a_page_is_handed_the_file_rather_than_a_picture() {
        let played = parsed(&played_document("holiday.mp4"));
        let first = &played["form"]["children"][0];
        assert_eq!(first["t"], "media");
        assert_eq!(first["media"], "video");
        assert_eq!(first["src"], "file:holiday.mp4");
        assert_eq!(first["autoplay"], true);
    }

    #[test]
    fn a_terminal_has_nowhere_to_show_a_film() {
        let host = ic_plugin_api::testing::silent_host();
        let console = CString::new(ic_plugin_api::IC_HOST_CONSOLE).expect("a kind");
        assert_eq!(ic_plugin_init(&host, console.as_ptr()), IC_ERR_NOT_THIS_HOST);
    }

    #[test]
    fn pressing_a_window_that_has_no_player_does_nothing() {
        SHOWING.with(|held| {
            held.borrow_mut().insert(
                7,
                Showing {
                    named: "holiday.mp4".to_string(),
                    local: None,
                    player: None,
                    trouble: None,
                    dragged_to: None,
                },
            )
        });
        let event = serde_json::json!({ "node": "play", "instance": 7 }).to_string();
        let answered = viewer_event(event.as_ptr(), event.len() as u64, std::ptr::null_mut());
        assert_eq!(
            String::from_utf8_lossy(unsafe {
                std::slice::from_raw_parts(answered.data, answered.len as usize)
            }),
            "{}"
        );
        viewer_closed(7, std::ptr::null_mut());
    }
}
