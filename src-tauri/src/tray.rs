// The tray icon — the copilot's wake-up entry.
//
// Why this exists: ADR-0047 decision 5 puts the wake-up path on the copilot's
// own tray rather than on the desktop app's reopen hook. The desktop app went
// back to "closing the window ends the process", so it has no hidden-window
// state to bring back; the copilot is the resident one.
//
// The single hard reason for choosing Tauri over Wails was the tray: Wails v2
// has no tray at all (`options.App` has no Tray field, `pkg/menu`'s `TrayMenu`
// is a dead type), and the third-party systray crates need cgo, which breaks
// the bare-ubuntu `go build ./...` in the main repository's CI.
//
// What the tray does here is deliberately small:
//   · left click  → show and focus the chat window (this is the whole point)
//   · right click → Show Chat / Settings / Show-Hide Companion / Quit
//   · ⌥ Space     → the same toggle, from anywhere on the machine
//
// Warden is a status-bar resident, not a windowed application: on macOS it
// runs as an Accessory so closing the last surface does not end the process
// and does not leave a Dock icon behind. The chat window is headless — a
// borderless overlay that hides itself when it loses focus. The desk pet is
// optional: the tray can hide it while the app stays resident.
//
// Only Quit exits. If the tray failed, another visible surface must remain
// reachable (see `hide_surface`).

use tauri::{
    AppHandle, Emitter, Manager, Runtime,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

/// The tray icon's id. Also the key for `AppHandle::tray_by_id`.
const TRAY_ID: &str = "main";

/// Menu item ids. They are matched by string in the event handler, so they
/// are declared as constants rather than spelled out at the call sites.
const MENU_SHOW: &str = "show";
const MENU_SETTINGS: &str = "settings";
const MENU_COMPANION: &str = "toggle-companion";
const MENU_QUIT: &str = "quit";

/// The global wake-up shortcut. ⌥ Space is free on macOS (Spotlight uses
/// ⌘ Space) and does not collide with common editor bindings.
pub const SHORTCUT: &str = "Alt+Space";

/// The event that tells the main window which page to show.
pub const NAVIGATE_EVENT: &str = "warden:navigate";

/// Whether the desk pet is visible. Stored next to the other configuration;
/// a missing or unreadable file means the default — visible.
const PREFS_FILE: &str = "preferences.json";

fn prefs_path<R: Runtime>(app: &AppHandle<R>) -> Option<std::path::PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join(PREFS_FILE))
}

pub fn companion_visible<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(path) = prefs_path(app) else {
        return true;
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| value["companionVisible"].as_bool())
        .unwrap_or(true)
}

fn set_companion_visible<R: Runtime>(app: &AppHandle<R>, visible: bool) {
    let Some(path) = prefs_path(app) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        path,
        serde_json::json!({ "companionVisible": visible }).to_string(),
    );
}

/// The macOS tray icon: a template image.
///
/// macOS menu bar icons are template images — a single colour on a
/// transparent field, which the system recolours for light and dark menu
/// bars. The file is Warden's lantern cut to one colour — the same glyph
/// domain the VS Code activity bar and the JetBrains stripe icon use, but
/// from `build/icon-warden.svg`, not Cinder's letterform. A coloured app
/// icon would look wrong in a menu bar and would not adapt.
///
/// `icon_as_template` is macOS-only (the `tray-icon` crate ignores it
/// elsewhere), so the non-macOS platforms get the coloured app icon instead
/// — a black glyph on a dark taskbar is invisible.
///
/// SVG is not an option: `tauri::image::Image::from_bytes` decodes through
/// the `image` crate, which reads PNG and ICO only.
///
/// The size is 22px, not 44px. The macOS backend of the `tray-icon` crate
/// scales the image to a fixed 18 points tall and derives the width from
/// the aspect ratio (`tray-icon-0.25.1/src/platform_impl/macos/mod.rs`), so
/// a 44px file would be drawn at 44 points — twice the menu bar height.
#[cfg(target_os = "macos")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-22.png");

/// The Windows and Linux tray icon: the app icon, in colour.
///
/// These platforms render the tray icon as-is on the taskbar or panel, and
/// they have no template-image mechanism to recolour it. The squircle app
/// icon stays legible on any taskbar background.
#[cfg(not(target_os = "macos"))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/32x32.png");

/// Build the tray icon and attach it to the app.
///
/// Called from the builder's setup hook, so the tray exists before the user
/// can possibly look for it — a tray that appears a moment after the window
/// is the kind of detail that make people conclude the feature is broken.
///
/// The menu is built inside `run_on_main_thread` because AppKit menus may
/// only be created on the main thread (`muda` enforces this with a panic).
/// The real runtime runs that closure on the main thread; the test runtime
/// only queues it, which is why the unit test can assert the *registration*
/// but not the AppKit object itself.
///
/// A failure inside the closure cannot be propagated out — `run_on_main_thread`
/// takes a `FnOnce() -> ()`. So it is logged instead. Silently swallowing it
/// would be the worst outcome: the app would look fine and the wake-up path
/// would simply not exist, and the user would have no way to tell that from
/// "the tray is there but I did not find it". The first screen reports the
/// tray's presence, so a missing tray is visible rather than mysterious.
pub fn install<R: Runtime>(app: &mut tauri::App<R>) -> tauri::Result<()> {
    let handle = app.handle().clone();
    let icon = tauri::image::Image::from_bytes(TRAY_ICON)?;

    let inner = handle.clone();
    handle.run_on_main_thread(move || match install_on_main_thread(&inner, icon) {
        Ok(()) => eprintln!("[tray] the tray icon is installed"),
        Err(err) => eprintln!("[tray] the tray icon could not be installed: {err}"),
    })?;

    Ok(())
}

/// Build the tray on the main thread, where AppKit allows it.
fn install_on_main_thread<R: Runtime>(
    handle: &AppHandle<R>,
    icon: tauri::image::Image<'static>,
) -> tauri::Result<()> {
    let show = MenuItem::with_id(handle, MENU_SHOW, "Show Chat", true, None::<&str>)?;
    let settings = MenuItem::with_id(handle, MENU_SETTINGS, "Settings", true, None::<&str>)?;
    let companion = MenuItem::with_id(
        handle,
        MENU_COMPANION,
        "Show / Hide Companion",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(handle, MENU_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(handle, &[&show, &settings, &companion, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Warden")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SHOW => show_chat(app),
            MENU_SETTINGS => show_settings(app),
            MENU_COMPANION => toggle_companion(app),
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left click is the wake-up path. Right click opens the menu
            // instead, which is why the menu is not shown on left click.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_chat(tray.app_handle());
            }
        })
        .build(handle)?;

    Ok(())
}

/// Whether the tray icon is present.
///
/// This is not a decorative query. The tray is the copilot's only wake-up
/// path (ADR-0047 decision 5), and macOS gives no way to ask another
/// process's status items from the outside — `NSStatusBar` only exposes
/// the current process's items, and the status bar itself is rendered by
/// the system. So "is the tray really there?" cannot be answered by
/// looking at the screen from another process; it has to be answered from
/// inside the app.
///
/// The first screen shows it, because a user who cannot find the tray has
/// no way to tell a missing tray from a tray that failed to install.
pub fn is_installed<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.tray_by_id(TRAY_ID).is_some()
}

/// Bring the chat window back, on the chat page.
///
/// `show` alone is not enough: the window may still be minimized from an
/// earlier hide, and a shown-but-minimized window reads as "the app did
/// nothing". `unminimize` first, then `set_focus`, so the keystroke lands
/// where the user is looking.
pub fn show_chat<R: Runtime>(app: &AppHandle<R>) {
    set_policy(app, false);
    if let Err(error) = show_surface(app, "main", true) {
        eprintln!("[window] {error}");
        return;
    }
    let _ = app.emit(NAVIGATE_EVENT, "chat");
}

/// Open the same window on its settings page. The tray's right-click menu
/// is one of the two ways in; the quiet button in the chat header is the
/// other. Both go through one view switch, so the two cannot disagree.
pub fn show_settings<R: Runtime>(app: &AppHandle<R>) {
    set_policy(app, false);
    if let Err(error) = show_surface(app, "main", true) {
        eprintln!("[window] {error}");
        return;
    }
    let _ = app.emit(NAVIGATE_EVENT, "settings");
}

/// Toggle the chat window from the global shortcut.
///
/// A shortcut that only ever shows is awkward: half the time the reason to
/// press it is to put the overlay away. Focus is the state to test, not
/// visibility — a visible but unfocused window still needs bringing forward.
pub fn toggle_chat<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_focused().unwrap_or(false) {
        hide_chat(app);
    } else {
        show_chat(app);
    }
}

/// Put the chat overlay away and return to a pure status-bar presence.
pub fn hide_chat<R: Runtime>(app: &AppHandle<R>) {
    if let Err(error) = hide_surface(app, "main") {
        eprintln!("[window] {error}");
        return;
    }
    set_policy(app, true);
}

/// Show or hide the desk pet, remembering the choice.
///
/// The tray is the resident surface; the pet is decoration the user may not
/// want on screen. Hiding it must not end the app or disable ⌥ Space.
pub fn toggle_companion<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("companion") else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    let result = if visible {
        window.hide().map_err(|e| e.to_string())
    } else {
        show_surface(app, "companion", false)
    };
    if let Err(error) = result {
        eprintln!("[window] {error}");
        return;
    }
    set_companion_visible(app, !visible);
}

/// Apply the saved desk-pet preference at startup. The window is created
/// visible by the config; this is the one place that can override it.
pub fn apply_companion_preference<R: Runtime>(app: &AppHandle<R>) {
    if companion_visible(app) {
        return;
    }
    if let Some(window) = app.get_webview_window("companion")
        && let Err(error) = window.hide()
    {
        eprintln!("[window] {error}");
    }
}

pub fn check_surface(label: &str) -> Result<(), String> {
    if matches!(label, "main" | "companion") {
        Ok(())
    } else {
        Err("only Warden's main and companion windows may be controlled".into())
    }
}

/// macOS: a pure Accessory app cannot become the active application, so a
/// borderless window shown under that policy would not receive keystrokes.
/// Becoming Regular while the chat overlay is up fixes that; going back to
/// Accessory when it is put away removes the Dock icon again. Other platforms
/// have no activation policy to change.
fn set_policy<R: Runtime>(app: &AppHandle<R>, accessory: bool) {
    #[cfg(target_os = "macos")]
    {
        let policy = if accessory {
            tauri::ActivationPolicy::Accessory
        } else {
            tauri::ActivationPolicy::Regular
        };
        if let Err(error) = app.set_activation_policy(policy) {
            eprintln!("[window] activation policy: {error}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, accessory);
}

pub fn show_surface<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    focus: bool,
) -> Result<(), String> {
    check_surface(label)?;
    let window = app
        .get_webview_window(label)
        .ok_or_else(|| format!("Warden {label} window is unavailable"))?;
    window.unminimize().map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    if focus {
        window.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn hide_surface<R: Runtime>(app: &AppHandle<R>, label: &str) -> Result<(), String> {
    check_surface(label)?;
    let window = app
        .get_webview_window(label)
        .ok_or_else(|| format!("Warden {label} window is unavailable"))?;
    if !is_installed(app) {
        let other_label = if label == "main" { "companion" } else { "main" };
        // A failed tray must not leave an invisible, unreachable process.
        // Showing the fallback never focuses it. Failure leaves this window up.
        show_surface(app, other_label, false)
            .map_err(|e| format!("cannot hide {label}: no wake-up entry is available ({e})"))?;
        let other = app
            .get_webview_window(other_label)
            .ok_or("fallback window disappeared")?;
        if !other.is_visible().unwrap_or(false) || other.is_minimized().unwrap_or(true) {
            return Err(format!(
                "cannot hide {label}: the fallback window is not visible"
            ));
        }
    }
    window
        .hide()
        .map_err(|e| format!("could not hide {label}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces_survive_hiding_and_require_a_wake_up_entry() {
        let app = tauri::test::mock_app();
        let main = tauri::WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default())
            .build()
            .unwrap();
        assert!(
            hide_surface(app.handle(), "main")
                .unwrap_err()
                .contains("no wake-up entry")
        );
        assert!(app.get_webview_window("main").is_some());
        let companion =
            tauri::WebviewWindowBuilder::new(&app, "companion", tauri::WebviewUrl::default())
                .build()
                .unwrap();
        hide_surface(app.handle(), "main").unwrap();
        hide_surface(app.handle(), "companion").unwrap();
        assert!(app.get_webview_window("main").is_some());
        assert!(app.get_webview_window("companion").is_some());
        show_surface(app.handle(), "main", false).unwrap();
        assert!(show_surface(app.handle(), "untrusted", false).is_err());
        crate::window_drag(main).unwrap();
        crate::window_drag(companion).unwrap();
        let other =
            tauri::WebviewWindowBuilder::new(&app, "untrusted", tauri::WebviewUrl::default())
                .build()
                .unwrap();
        assert!(crate::window_drag(other).is_err());
        // MockRuntime doesn't implement visibility changes; this tests the
        // hide path and retained handles, not actual OS rendering.
    }

    #[test]
    fn window_config_and_capabilities_are_restricted_to_two_local_surfaces() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let windows = config["app"]["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 2);
        let main = &windows[0];
        assert_eq!(
            (main["width"].as_u64(), main["height"].as_u64()),
            (Some(480), Some(760))
        );
        assert_eq!(
            (main["minWidth"].as_u64(), main["minHeight"].as_u64()),
            (Some(380), Some(520))
        );
        // The chat window is a headless overlay: hidden at startup, borderless,
        // transparent and out of the taskbar. Only the desk pet is visible.
        for flag in ["visible", "decorations", "focus"] {
            assert_eq!(main[flag], false, "main.{flag}");
        }
        for flag in ["transparent", "skipTaskbar", "shadow"] {
            assert_eq!(main[flag], true, "main.{flag}");
        }
        let pet = &windows[1];
        assert_eq!(pet["label"], "companion");
        assert_eq!(pet["url"], "index.html?surface=companion");
        assert_eq!(pet["width"], 168);
        assert_eq!(pet["height"], 196);
        for flag in ["alwaysOnTop", "transparent", "skipTaskbar"] {
            assert_eq!(pet[flag], true);
        }
        for flag in ["decorations", "resizable", "focus"] {
            assert_eq!(pet[flag], false);
        }
        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(caps["windows"], serde_json::json!(["main", "companion"]));
        assert!(caps.get("remote").is_none());
        assert_eq!(caps["permissions"], serde_json::json!(["core:default"]));
    }

    /// The tray's whole reason to exist is the wake-up path. If the icon id
    /// drifts away from the one the config or the lookup uses, the tray
    /// silently becomes an unreachable decoration.
    ///
    /// This cannot build a real tray (it needs a running app), so what it
    /// pins is the contract: the id and the menu item ids are the ones the
    /// docs and the rest of the crate refer to.
    #[test]
    fn tray_ids_are_the_documented_ones() {
        assert_eq!(TRAY_ID, "main");
        assert_eq!(MENU_SHOW, "show");
        assert_eq!(MENU_SETTINGS, "settings");
        assert_eq!(MENU_COMPANION, "toggle-companion");
        assert_eq!(MENU_QUIT, "quit");
        assert_eq!(SHORTCUT, "Alt+Space");
        assert_eq!(NAVIGATE_EVENT, "warden:navigate");
    }

    /// The tray icon must be a real PNG of the size the platform expects.
    ///
    /// `include_bytes!` fails at compile time if a file is missing, but not if
    /// it is the wrong size or the wrong format — and a 0-byte or non-PNG file
    /// would only fail at runtime, inside the user's menu bar.
    ///
    /// The expected size differs per platform: macOS scales the image to 18
    /// points tall, so a 22px source is right; Windows and Linux draw the
    /// app icon at its own size.
    #[test]
    fn tray_icon_is_a_png_of_the_expected_size() {
        let expected: u32 = if cfg!(target_os = "macos") { 22 } else { 32 };

        assert!(TRAY_ICON.len() > 8, "tray icon must not be empty");
        assert_eq!(
            &TRAY_ICON[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            "tray icon must be a PNG: Tauri decodes PNG and ICO only"
        );

        // IHDR carries the width and height right after the PNG signature
        // (8 bytes) and the IHDR chunk header (4 length bytes + 4 "IHDR").
        let width = u32::from_be_bytes(TRAY_ICON[16..20].try_into().expect("4 bytes"));
        let height = u32::from_be_bytes(TRAY_ICON[20..24].try_into().expect("4 bytes"));
        assert_eq!(width, expected, "tray icon width");
        assert_eq!(height, expected, "tray icon height");
    }
}
