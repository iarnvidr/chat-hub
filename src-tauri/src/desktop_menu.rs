use tauri::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Manager, Runtime,
};

const CLOSE_TAB_MENU_ID: &str = "chat-hub-close-tab";

#[cfg(target_os = "macos")]
const CLOSE_TAB_ACCELERATOR: &str = "Cmd+W";
#[cfg(not(target_os = "macos"))]
const CLOSE_TAB_ACCELERATOR: &str = "Ctrl+W";

#[cfg(target_os = "macos")]
const NEW_TAB_ACCELERATOR: &str = "Cmd+T";
#[cfg(not(target_os = "macos"))]
const NEW_TAB_ACCELERATOR: &str = "Ctrl+T";

#[cfg(target_os = "macos")]
const PREVIOUS_TAB_ACCELERATOR: &str = "Ctrl+Shift+Tab";
#[cfg(not(target_os = "macos"))]
const PREVIOUS_TAB_ACCELERATOR: &str = "Ctrl+Shift+Tab";
#[cfg(target_os = "macos")]
const NEXT_TAB_ACCELERATOR: &str = "Ctrl+Tab";
#[cfg(not(target_os = "macos"))]
const NEXT_TAB_ACCELERATOR: &str = "Ctrl+Tab";

#[cfg(target_os = "macos")]
const TAB_NUMBER_MODIFIER: &str = "Cmd";
#[cfg(not(target_os = "macos"))]
const TAB_NUMBER_MODIFIER: &str = "Ctrl";

/// Preserve Tauri's standard app menus, replacing only its window-close item
/// with the application-level close-tab action where a File menu exists.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    let file_menu = menu.items()?.into_iter().find_map(|item| {
        let submenu = item.as_submenu()?;
        (submenu.text().ok().as_deref() == Some("File")).then(|| submenu.clone())
    });

    if let Some(file_menu) = file_menu {
        let close_window = file_menu.items()?.into_iter().find_map(|item| {
            let predefined = item.as_predefined_menuitem()?;
            (predefined.text().ok().as_deref() == Some("Close Window")).then(|| predefined.clone())
        });
        if let Some(close_window) = close_window {
            file_menu.remove(&close_window)?;
        }
        let close_tab = MenuItem::with_id(
            app,
            CLOSE_TAB_MENU_ID,
            "Close Tab",
            true,
            Some(CLOSE_TAB_ACCELERATOR),
        )?;
        file_menu.append(&close_tab)?;
    }

    let new_tab = MenuItem::with_id(
        app,
        "chat-hub-new-tab",
        "New Tab",
        true,
        Some(NEW_TAB_ACCELERATOR),
    )?;
    let previous = MenuItem::with_id(
        app,
        "chat-hub-previous-tab",
        "Previous Tab",
        true,
        Some(PREVIOUS_TAB_ACCELERATOR),
    )?;
    let next = MenuItem::with_id(
        app,
        "chat-hub-next-tab",
        "Next Tab",
        true,
        Some(NEXT_TAB_ACCELERATOR),
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let mut number_items = Vec::new();
    for number in 1..=9 {
        let id = format!("chat-hub-select-tab-{number}");
        let accelerator = format!("{TAB_NUMBER_MODIFIER}+{number}");
        let label = if number == 9 {
            "Last Tab".to_string()
        } else {
            format!("Select Tab {number}")
        };
        number_items.push(MenuItem::with_id(app, id, label, true, Some(accelerator))?);
    }
    let mut tab_items: Vec<&dyn tauri::menu::IsMenuItem<R>> =
        vec![&new_tab, &previous, &next, &separator];
    tab_items.extend(
        number_items
            .iter()
            .map(|item| item as &dyn tauri::menu::IsMenuItem<R>),
    );
    let tabs_menu =
        Submenu::with_id_and_items(app, "chat-hub-tabs-menu", "Tabs", true, &tab_items)?;
    menu.append(&tabs_menu)?;

    Ok(menu)
}

/// Native menu events only dispatch a fixed local event. The UI decides
/// whether a tab exists; with no tabs the action intentionally does nothing.
pub fn handle<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let (event_name, detail) = match event.id().as_ref() {
        CLOSE_TAB_MENU_ID => ("chat-hub:menu-close-tab", None),
        "chat-hub-new-tab" => ("chat-hub:menu-new-tab", None),
        "chat-hub-previous-tab" => ("chat-hub:menu-previous-tab", None),
        "chat-hub-next-tab" => ("chat-hub:menu-next-tab", None),
        id if id.starts_with("chat-hub-select-tab-") => {
            let number = &id["chat-hub-select-tab-".len()..];
            if !(1..=9).any(|n| number == n.to_string()) {
                return;
            }
            ("chat-hub:menu-select-tab", Some(number.to_string()))
        }
        _ => return,
    };
    let script = match detail {
        Some(number) => {
            format!("window.dispatchEvent(new CustomEvent('{event_name}', {{detail: {number}}}))")
        }
        None => format!("window.dispatchEvent(new Event('{event_name}'))"),
    };
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.eval(&script);
    }
}
