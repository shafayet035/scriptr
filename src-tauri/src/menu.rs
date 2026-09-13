//! Native menu bar. Every custom item emits the `menu` event with its id.

use tauri::menu::{AboutMetadata, Menu, MenuBuilder, MenuItem, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Runtime};

pub const EVENT: &str = "menu";

/// Custom item ids understood by the frontend.
const IDS: &[&str] = &[
    "settings",
    "add-project",
    "import-toml",
    "export-toml",
    "close-tab",
    "palette",
    "view-terminals",
    "view-deps",
    "view-log",
    "find",
    "toggle-inspector",
    "run-group",
    "stop-group",
    "restart-script",
    "stop-everything",
];

pub fn is_custom(id: &str) -> bool {
    IDS.contains(&id)
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let item = |id: &str, text: &str, accelerator: Option<&str>| -> tauri::Result<MenuItem<R>> {
        let builder = MenuItemBuilder::with_id(id, text);
        match accelerator {
            Some(a) => builder.accelerator(a),
            None => builder,
        }
        .build(app)
    };

    let app_menu = SubmenuBuilder::new(app, "Scriptr")
        .about(Some(AboutMetadata { name: Some("Scriptr".into()), ..Default::default() }))
        .separator()
        .item(&item("settings", "Settings…", Some("CmdOrCtrl+,"))?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let file = SubmenuBuilder::new(app, "File")
        .item(&item("add-project", "Add Project…", Some("CmdOrCtrl+O"))?)
        .separator()
        .item(&item("import-toml", "Import scriptr.toml…", None)?)
        .item(&item("export-toml", "Export scriptr.toml…", None)?)
        .separator()
        .item(&item("close-tab", "Close Tab", Some("CmdOrCtrl+W"))?)
        .build()?;

    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let view = SubmenuBuilder::new(app, "View")
        .item(&item("palette", "Command Palette", Some("CmdOrCtrl+K"))?)
        .separator()
        .item(&item("view-terminals", "Terminals", Some("CmdOrCtrl+1"))?)
        .item(&item("view-deps", "Dependencies", Some("CmdOrCtrl+2"))?)
        .item(&item("view-log", "Combined Log", Some("CmdOrCtrl+3"))?)
        .separator()
        .item(&item("find", "Find in Terminal", Some("CmdOrCtrl+F"))?)
        .item(&item("toggle-inspector", "Toggle Inspector", Some("CmdOrCtrl+I"))?)
        .separator()
        .fullscreen()
        .build()?;

    let run = SubmenuBuilder::new(app, "Run")
        .item(&item("run-group", "Run Group", Some("CmdOrCtrl+R"))?)
        .item(&item("stop-group", "Stop Group", Some("CmdOrCtrl+."))?)
        .item(&item("restart-script", "Restart Script", Some("CmdOrCtrl+Shift+R"))?)
        .separator()
        .item(&item("stop-everything", "Stop Everything", Some("CmdOrCtrl+Shift+."))?)
        .build()?;

    // `maximize` is "Zoom" on macOS.
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .bring_all_to_front()
        .build()?;

    let help = SubmenuBuilder::new(app, "Help").build()?;

    MenuBuilder::new(app).items(&[&app_menu, &file, &edit, &view, &run, &window, &help]).build()
}
