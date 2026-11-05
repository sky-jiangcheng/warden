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
//   · left click  → show and focus the window (this is the whole point)
//   · right click → a menu with "Show" and "Quit"
//
// Closing the window ends the app, the same as the desktop app. The tray is
// for waking up an app that is already running — a window that hides itself
// on close would make the tray the only way back in, and the copilot's
// window is its whole UI. Hiding it would trade one unreachable window for
// an unreachable process.

use tauri::{
    AppHandle, Manager, Runtime,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

/// The tray icon's id. Also the key for `AppHandle::tray_by_id`.
const TRAY_ID: &str = "main";

/// Menu item ids. They are matched by string in the event handler, so they
/// are declared as constants rather than spelled out at the call sites.
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";

/// The macOS tray icon: a template image.
///
/// macOS menu bar icons are template images — a single colour on a
/// transparent field, which the system recolours for light and dark menu
/// bars. The file is the letterform of Repotory's brand mark cut to one
/// colour: the same glyph domain the VS Code activity bar and the JetBrains
/// stripe icon use. A coloured app icon would look wrong in a menu bar and
/// would not adapt.
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
    let show = MenuItem::with_id(handle, MENU_SHOW, "Show Repotory Agent", true, None::<&str>)?;
    let quit = MenuItem::with_id(handle, MENU_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(handle, &[&show, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Repotory Agent")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_SHOW => show_window(app),
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
                show_window(tray.app_handle());
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

/// Bring the copilot's window back.
///
/// `show` alone is not enough: the window may still be minimized from an
/// earlier hide, and a shown-but-minimized window reads as "the app did
/// nothing". `unminimize` first, then `set_focus`, so the keystroke lands
/// where the user is looking.
fn show_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        // No window to bring back. The tray still works as a menu (Quit is
        // still reachable), so this is a degraded state, not a broken one.
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(MENU_QUIT, "quit");
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
