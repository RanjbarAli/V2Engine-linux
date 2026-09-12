mod config;
use config::{Server, Store};
use gtk::{gdk, glib, prelude::*};
use ksni::blocking::TrayMethods;
use std::{
    cell::RefCell,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    process::{Command, Stdio},
    rc::Rc,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const APP_ID: &str = "io.github.ranjbarali.V2Engine";

fn main() {
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|app| {
        style(app);
        let _ = V2EngineTray.assume_sni_available(true).spawn();
    });
    app.connect_activate(build);
    app.run();
}

#[derive(Debug)]
struct V2EngineTray;

fn open_main_window() {
    if let Ok(executable) = std::env::current_exe() {
        let _ = Command::new(executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

fn tray_toggle_connection() {
    let args = if system_connected() {
        vec!["stop".to_string()]
    } else {
        let store = config::load();
        let Some(server) = store
            .servers
            .iter()
            .find(|server| Some(&server.id) == store.selected.as_ref())
        else {
            open_main_window();
            return;
        };
        let Ok(path) = runtime_config(server, &store.bypass) else {
            open_main_window();
            return;
        };
        vec!["start".to_string(), path.to_string_lossy().into_owned()]
    };
    let transient = args.get(1).map(PathBuf::from);
    let _ = Command::new("pkexec")
        .arg("/usr/lib/v2engine/v2engine-helper")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Some(path) = transient {
        let _ = fs::remove_file(path);
    }
}

impl ksni::Tray for V2EngineTray {
    fn id(&self) -> String {
        "v2engine".into()
    }

    fn title(&self) -> String {
        if system_connected() {
            "V2Engine · Connected".into()
        } else {
            "V2Engine · Disconnected".into()
        }
    }

    fn icon_name(&self) -> String {
        APP_ID.into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_main_window();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{StandardItem, SubMenu};
        let store = config::load();
        let selected = store.selected.clone();
        let server_items = if store.servers.is_empty() {
            vec![StandardItem {
                label: "No servers added".into(),
                enabled: false,
                ..Default::default()
            }
            .into()]
        } else {
            store
                .servers
                .into_iter()
                .map(|server| {
                    let id = server.id.clone();
                    StandardItem {
                        label: if selected.as_deref() == Some(&server.id) {
                            format!("✓  {}", server.name)
                        } else {
                            server.name
                        },
                        activate: Box::new(move |_| {
                            let mut store = config::load();
                            store.selected = Some(id.clone());
                            let _ = config::save(&store);
                        }),
                        ..Default::default()
                    }
                    .into()
                })
                .collect()
        };
        let connected = system_connected();
        let can_start = connected || selected.is_some();
        vec![
            StandardItem {
                label: "Open V2Engine".into(),
                icon_name: "window-new-symbolic".into(),
                activate: Box::new(|_| open_main_window()),
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: "Select Server".into(),
                icon_name: "network-server-symbolic".into(),
                submenu: server_items,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if connected {
                    "Stop · Connected".into()
                } else {
                    "Start · Disconnected".into()
                },
                enabled: can_start,
                icon_name: "system-shutdown-symbolic".into(),
                activate: Box::new(|_| {
                    thread::spawn(tray_toggle_connection);
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(|_| {
                    thread::spawn(|| {
                        if system_connected() {
                            tray_toggle_connection();
                        }
                        std::process::exit(0);
                    });
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn style(_: &gtk::Application) {
    let css = gtk::CssProvider::new();
    css.load_from_data(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().unwrap(),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

struct Ui {
    store: Store,
    list: gtk::ListBox,
    toggle: gtk::ToggleButton,
    status: gtk::Label,
    status_dot: gtk::Box,
    connection_detail: gtk::Label,
    current_ip: gtk::Label,
    protocol_value: gtk::Label,
    upload: gtk::Label,
    download: gtk::Label,
    duration: gtk::Label,
    server_stack: gtk::Stack,
    server_panel: gtk::Box,
    empty_instruction: gtk::Label,
    direct_list: gtk::ListBox,
    connected_since: Option<Instant>,
    busy: bool,
}

fn styled_label(text: &str, class: &str, xalign: f32) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class(class);
    label.set_xalign(xalign);
    label
}

fn metric_item(caption: &str) -> (gtk::Box, gtk::Label) {
    let item = gtk::Box::new(gtk::Orientation::Vertical, 3);
    item.add_css_class("metric-item");
    item.append(&styled_label(caption, "metric-caption", 0.0));
    let value = styled_label("—", "metric-value", 0.0);
    item.append(&value);
    (item, value)
}

fn action_button(label: &str, icon: &str) -> gtk::Button {
    let button = gtk::Button::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    content.set_halign(gtk::Align::Center);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(17);
    content.append(&image);
    content.append(&gtk::Label::new(Some(label)));
    button.set_child(Some(&content));
    button
}

fn nav_button(label: &str, icon: &str) -> gtk::Button {
    let button = action_button(label, icon);
    button.add_css_class("nav-item");
    button
}

fn protocol_label(server: &Server) -> String {
    if server.protocol.eq_ignore_ascii_case("vless")
        && server.uri.to_ascii_lowercase().contains("security=reality")
    {
        "VLESS · Reality".into()
    } else {
        server.protocol.clone()
    }
}

fn country_flag(name: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    if name.contains("frankfurt") || name.contains("germany") || name.contains("🇩🇪") {
        "🇩🇪"
    } else if name.contains("paris") || name.contains("france") || name.contains("🇫🇷") {
        "🇫🇷"
    } else if name.contains("london") || name.contains("united kingdom") || name.contains("🇬🇧")
    {
        "🇬🇧"
    } else if name.contains("new york") || name.contains("united states") || name.contains("🇺🇸")
    {
        "🇺🇸"
    } else if name.contains("amsterdam") || name.contains("netherlands") || name.contains("🇳🇱")
    {
        "🇳🇱"
    } else if name.contains("istanbul") || name.contains("turkey") || name.contains("🇹🇷") {
        "🇹🇷"
    } else if name.contains("helsinki") || name.contains("finland") || name.contains("🇫🇮") {
        "🇫🇮"
    } else if name.contains("singapore") || name.contains("🇸🇬") {
        "🇸🇬"
    } else if name.contains("tokyo") || name.contains("japan") || name.contains("🇯🇵") {
        "🇯🇵"
    } else {
        "◉"
    }
}

fn build(app: &gtk::Application) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("V2Engine")
        .default_width(980)
        .default_height(620)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("app-shell");

    let header = gtk::HeaderBar::new();
    header.add_css_class("app-header");
    header.set_show_title_buttons(false);
    header.set_title_widget(Some(&gtk::Label::new(None)));
    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    brand.set_valign(gtk::Align::Center);
    let brand_mark = styled_label("V", "brand-mark", 0.5);
    brand_mark.set_valign(gtk::Align::Center);
    brand.append(&brand_mark);
    brand.append(&styled_label("V2Engine", "wordmark", 0.0));
    header.pack_start(&brand);
    let window_controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    window_controls.add_css_class("window-controls");
    window_controls.set_valign(gtk::Align::Center);
    let minimize = gtk::Button::new();
    minimize.add_css_class("window-control");
    minimize.add_css_class("minimize-control");
    minimize.set_tooltip_text(Some("Minimize"));
    let maximize = gtk::Button::new();
    maximize.add_css_class("window-control");
    maximize.add_css_class("maximize-control");
    maximize.set_tooltip_text(Some("Maximize"));
    let close = gtk::Button::new();
    close.add_css_class("window-control");
    close.add_css_class("close-control");
    close.set_tooltip_text(Some("Close"));
    window_controls.append(&minimize);
    window_controls.append(&maximize);
    window_controls.append(&close);

    header.pack_end(&window_controls);
    root.append(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 14);
    body.set_margin_start(20);
    body.set_margin_end(20);
    body.set_margin_top(10);
    body.set_margin_bottom(16);
    body.set_vexpand(true);

    let workspace = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    workspace.set_vexpand(true);
    workspace.set_homogeneous(true);

    let left_column = gtk::Box::new(gtk::Orientation::Vertical, 14);
    let connection_panel = gtk::Box::new(gtk::Orientation::Vertical, 18);
    connection_panel.add_css_class("panel");
    connection_panel.add_css_class("connection-panel");
    let connection_top = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    let connection_copy = gtk::Box::new(gtk::Orientation::Vertical, 7);
    connection_copy.set_hexpand(true);
    let state_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let state_dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    state_dot.add_css_class("state-dot");
    let status = gtk::Label::new(Some("Disconnected"));
    status.add_css_class("status");
    status.set_xalign(0.0);
    state_line.append(&state_dot);
    state_line.append(&status);
    let connection_detail = styled_label("No server selected", "connection-detail", 0.0);
    connection_detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
    connection_copy.append(&state_line);
    connection_copy.append(&connection_detail);

    let toggle = gtk::ToggleButton::new();
    toggle.add_css_class("power-button");
    toggle.set_size_request(68, 68);
    toggle.set_valign(gtk::Align::Center);
    toggle.set_tooltip_text(Some("Connect or disconnect"));
    let power = gtk::Image::from_icon_name("system-shutdown-symbolic");
    power.set_pixel_size(27);
    toggle.set_child(Some(&power));
    connection_top.append(&connection_copy);
    connection_top.append(&toggle);
    connection_panel.append(&connection_top);

    let metrics = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(12)
        .column_homogeneous(true)
        .build();
    metrics.add_css_class("metrics-grid");
    let (ip_item, current_ip) = metric_item("Current IP");
    let (protocol_item, protocol_value) = metric_item("Protocol");
    let (upload_item, upload) = metric_item("Upload");
    let (download_item, download) = metric_item("Download");
    let (duration_item, duration) = metric_item("Duration");
    metrics.attach(&ip_item, 0, 0, 1, 1);
    metrics.attach(&protocol_item, 1, 0, 1, 1);
    metrics.attach(&upload_item, 0, 1, 1, 1);
    metrics.attach(&download_item, 1, 1, 1, 1);
    metrics.attach(&duration_item, 0, 2, 2, 1);
    connection_panel.append(&metrics);
    left_column.append(&connection_panel);

    let direct_panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
    direct_panel.add_css_class("panel");
    direct_panel.add_css_class("direct-panel");
    direct_panel.set_vexpand(true);
    direct_panel.append(&styled_label("Direct Sites", "panel-title", 0.0));
    let direct_description = styled_label(
        "These domains bypass the proxy and use your normal connection.",
        "panel-description",
        0.0,
    );
    direct_description.set_wrap(true);
    direct_panel.append(&direct_description);
    let direct_list = gtk::ListBox::new();
    direct_list.set_selection_mode(gtk::SelectionMode::None);
    direct_list.add_css_class("direct-list");
    let direct_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .min_content_height(92)
        .child(&direct_list)
        .build();
    direct_scroll.add_css_class("flat-scroll");
    direct_panel.append(&direct_scroll);
    let add_domain = gtk::Button::with_label("+  Add Domain");
    add_domain.add_css_class("text-action");
    add_domain.set_halign(gtk::Align::Center);
    direct_panel.append(&add_domain);
    left_column.append(&direct_panel);

    let server_panel = gtk::Box::new(gtk::Orientation::Vertical, 14);
    server_panel.add_css_class("panel");
    server_panel.add_css_class("servers-panel");
    server_panel.set_vexpand(true);
    let server_header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let servers_title = styled_label("Servers", "panel-title", 0.0);
    servers_title.set_hexpand(true);
    let test = gtk::Button::with_label("Test All");
    test.add_css_class("compact-action");
    server_header.append(&servers_title);
    server_header.append(&test);
    server_panel.append(&server_header);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("server-list");
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(&list)
        .build();
    scroll.add_css_class("flat-scroll");

    let empty = gtk::Box::new(gtk::Orientation::Vertical, 9);
    empty.add_css_class("empty-state");
    empty.set_valign(gtk::Align::Center);
    empty.set_halign(gtk::Align::Center);
    let empty_icon = gtk::Image::from_icon_name("network-server-symbolic");
    empty_icon.set_pixel_size(30);
    empty_icon.add_css_class("empty-icon");
    empty.append(&empty_icon);
    empty.append(&styled_label("No servers added", "empty-title", 0.5));
    let empty_instruction = styled_label(
        "Press Ctrl+V or Add Server to import a configuration.",
        "empty-subtitle",
        0.5,
    );
    empty_instruction.set_wrap(true);
    empty_instruction.set_max_width_chars(38);
    empty.append(&empty_instruction);
    let add_server = gtk::Button::with_label("+  Add Server");
    add_server.add_css_class("primary-action");
    add_server.set_halign(gtk::Align::Center);
    empty.append(&add_server);

    let server_stack = gtk::Stack::new();
    server_stack.set_vexpand(true);
    server_stack.add_named(&empty, Some("empty"));
    server_stack.add_named(&scroll, Some("list"));
    server_panel.append(&server_stack);

    workspace.append(&left_column);
    workspace.append(&server_panel);
    body.append(&workspace);

    let navigation = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    navigation.add_css_class("bottom-nav");
    navigation.set_halign(gtk::Align::Center);
    let nav_servers = nav_button("Servers", "network-server-symbolic");
    nav_servers.add_css_class("nav-active");
    let nav_direct = nav_button("Direct Sites", "network-workgroup-symbolic");
    let nav_settings = nav_button("Settings", "preferences-system-symbolic");
    navigation.append(&nav_servers);
    navigation.append(&nav_direct);
    navigation.append(&nav_settings);
    body.append(&navigation);
    root.append(&body);
    window.set_child(Some(&root));
    let state = Rc::new(RefCell::new(Ui {
        store: config::load(),
        list: list.clone(),
        toggle: toggle.clone(),
        status: status.clone(),
        status_dot: state_dot.clone(),
        connection_detail: connection_detail.clone(),
        current_ip,
        protocol_value,
        upload,
        download,
        duration,
        server_stack: server_stack.clone(),
        server_panel: server_panel.clone(),
        empty_instruction: empty_instruction.clone(),
        direct_list: direct_list.clone(),
        connected_since: None,
        busy: false,
    }));
    sync_system_state(&state);
    populate_domains(&direct_list, &state);
    {
        let s = state.clone();
        list.connect_row_selected(move |_, row| {
            if let Some(r) = row {
                if let Some(id) = r.widget_name().strip_prefix("server-") {
                    let mut u = s.borrow_mut();
                    u.store.selected = Some(id.into());
                    if let Some(server) = u.store.servers.iter().find(|server| server.id == id) {
                        u.connection_detail.set_text(&format!(
                            "{} · {}",
                            server.name,
                            protocol_label(server)
                        ));
                    }
                    u.server_panel.remove_css_class("needs-attention");
                    let _ = config::save(&u.store);
                }
            }
        });
    }
    {
        let s = state.clone();
        let w = window.clone();
        toggle.connect_clicked(move |b| connect_toggle(&s, &w, b.is_active()));
    }
    {
        let s = state.clone();
        let w = window.clone();
        add_domain.connect_clicked(move |_| domain_dialog(&s, &w));
    }
    {
        let s = state.clone();
        test.connect_clicked(move |b| run_tests(&s, b));
    }
    {
        let s = state.clone();
        let w = window.clone();
        add_server.connect_clicked(move |_| import_dialog(&s, &w));
    }
    {
        let parent = window.clone();
        let servers = nav_servers.clone();
        let direct = nav_direct.clone();
        nav_settings.connect_clicked(move |button| {
            servers.remove_css_class("nav-active");
            direct.remove_css_class("nav-active");
            button.add_css_class("nav-active");
            settings_dialog(&parent);
        });
    }
    {
        let list = list.clone();
        let direct = nav_direct.clone();
        let settings = nav_settings.clone();
        nav_servers.connect_clicked(move |button| {
            direct.remove_css_class("nav-active");
            settings.remove_css_class("nav-active");
            button.add_css_class("nav-active");
            list.grab_focus();
        });
    }
    {
        let direct_list = direct_list.clone();
        let servers = nav_servers.clone();
        let settings = nav_settings.clone();
        nav_direct.connect_clicked(move |button| {
            servers.remove_css_class("nav-active");
            settings.remove_css_class("nav-active");
            button.add_css_class("nav-active");
            direct_list.grab_focus();
        });
    }
    {
        let window = window.clone();
        minimize.connect_clicked(move |_| window.minimize());
    }
    {
        let window = window.clone();
        maximize.connect_clicked(move |_| {
            if window.is_maximized() {
                window.unmaximize();
            } else {
                window.maximize();
            }
        });
    }
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.close());
    }
    {
        let state = state.clone();
        window.connect_map(move |_| sync_system_state(&state));
    }
    window.connect_close_request(|window| {
        window.set_visible(false);
        glib::Propagation::Stop
    });
    let keys = gtk::EventControllerKey::new();
    {
        let s = state.clone();
        let w = window.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if mods.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::v {
                import_clipboard(&s, &w);
                return glib::Propagation::Stop;
            }
            if mods.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::c {
                copy_selected(&s);
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::Delete {
                delete_selected(&s);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    window.add_controller(keys);
    window.present();
}

fn sync_system_state(s: &Rc<RefCell<Ui>>) {
    s.borrow_mut().store = config::load();
    refresh(s);
    let connected = system_connected();
    let mut u = s.borrow_mut();
    let mut timer = None;
    u.toggle.set_active(connected);
    if connected {
        u.status.set_text("Connected");
        u.status_dot.add_css_class("connected");
        if let Some(server) = u
            .store
            .servers
            .iter()
            .find(|server| Some(&server.id) == u.store.selected.as_ref())
        {
            let protocol = protocol_label(server);
            u.connection_detail
                .set_text(&format!("{} · {}", server.name, protocol));
            u.protocol_value.set_text(&protocol);
        }
        if u.connected_since.is_none() {
            let started = Instant::now();
            u.connected_since = Some(started);
            u.duration.set_text("00:00:00");
            timer = Some(started);
        }
    } else {
        u.status.set_text("Disconnected");
        u.status_dot.remove_css_class("connected");
        u.current_ip.set_text("—");
        u.protocol_value.set_text("—");
        u.upload.set_text("—");
        u.download.set_text("—");
        u.duration.set_text("—");
        u.connected_since = None;
    }
    drop(u);
    if let Some(started) = timer {
        start_duration_timer(s, started);
    }
}

fn refresh(s: &Rc<RefCell<Ui>>) {
    let u = s.borrow_mut();
    let list = u.list.clone();
    while let Some(c) = u.list.first_child() {
        u.list.remove(&c)
    }
    if u.store.servers.is_empty() {
        u.server_stack.set_visible_child_name("empty");
        u.connection_detail.set_text("No server selected");
        return;
    }
    u.server_stack.set_visible_child_name("list");
    let selected = u.store.selected.clone();
    let mut selected_row = None;
    for srv in &u.store.servers {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&format!("server-{}", srv.id));
        let grid = gtk::Grid::builder()
            .column_spacing(10)
            .row_spacing(3)
            .margin_top(11)
            .margin_bottom(11)
            .margin_start(12)
            .margin_end(12)
            .build();
        let flag = styled_label(country_flag(&srv.name), "server-flag", 0.5);
        let name = gtk::Label::new(Some(&srv.name));
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.add_css_class("server-name");
        let proto = gtk::Label::new(Some(&protocol_label(srv)));
        proto.set_xalign(0.0);
        proto.add_css_class("protocol");
        let latency_text = srv.latency.as_deref().unwrap_or("—");
        let latency_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        latency_box.set_valign(gtk::Align::Center);
        if latency_text == "Testing…" {
            let spinner = gtk::Spinner::new();
            spinner.set_spinning(true);
            spinner.set_size_request(14, 14);
            latency_box.append(&spinner);
        } else {
            let ping_dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            ping_dot.add_css_class("ping-dot");
            ping_dot.set_valign(gtk::Align::Center);
            if let Some(ms) = latency_text
                .strip_suffix(" ms")
                .and_then(|value| value.parse::<u64>().ok())
            {
                ping_dot.add_css_class(if ms <= 100 {
                    "ping-good"
                } else if ms <= 250 {
                    "ping-medium"
                } else {
                    "ping-poor"
                });
            } else if matches!(latency_text, "Failed" | "Timeout") {
                ping_dot.add_css_class("ping-poor");
            }
            latency_box.append(&ping_dot);
        }
        let latency = gtk::Label::new(Some(latency_text));
        latency.add_css_class("latency");
        if let Some(ms) = latency_text
            .strip_suffix(" ms")
            .and_then(|value| value.parse::<u64>().ok())
        {
            latency.add_css_class(if ms <= 100 {
                "latency-good"
            } else if ms <= 250 {
                "latency-medium"
            } else {
                "latency-poor"
            });
        } else if matches!(latency_text, "Failed" | "Timeout") {
            latency.add_css_class("latency-poor");
        }
        latency_box.append(&latency);
        let chevron = gtk::Image::from_icon_name("go-next-symbolic");
        chevron.set_pixel_size(13);
        chevron.add_css_class("server-chevron");
        grid.attach(&flag, 0, 0, 1, 2);
        grid.attach(&name, 1, 0, 1, 1);
        grid.attach(&proto, 1, 1, 1, 1);
        grid.attach(&latency_box, 2, 0, 1, 2);
        grid.attach(&chevron, 3, 0, 1, 2);
        row.set_child(Some(&grid));
        u.list.append(&row);
        if selected.as_deref() == Some(&srv.id) {
            selected_row = Some(row.clone());
            if !u.toggle.is_active() {
                u.connection_detail
                    .set_text(&format!("{} · {}", srv.name, protocol_label(srv)));
            }
        }
    }
    drop(u);
    if let Some(row) = selected_row {
        list.select_row(Some(&row));
    }
}

fn add_configs(s: &Rc<RefCell<Ui>>, text: &str) -> (usize, Vec<String>) {
    let (servers, bad) = config::parse_many(text);
    let mut u = s.borrow_mut();
    let before = u.store.servers.len();
    for server in servers {
        if !u.store.servers.iter().any(|item| item.id == server.id) {
            u.store.servers.push(server);
        }
    }
    if u.store.selected.is_none() {
        u.store.selected = u.store.servers.first().map(|server| server.id.clone());
    }
    let added = u.store.servers.len() - before;
    if added > 0 {
        u.server_panel.remove_css_class("needs-attention");
        u.empty_instruction
            .set_text("Press Ctrl+V or Add Server to import a configuration.");
        let _ = config::save(&u.store);
    }
    drop(u);
    if added > 0 {
        refresh(s);
    }
    (added, bad)
}

fn import_clipboard(s: &Rc<RefCell<Ui>>, _w: &gtk::ApplicationWindow) {
    let cb = gdk::Display::default().unwrap().clipboard();
    let s = s.clone();
    glib::MainContext::default().spawn_local(async move {
        match cb.read_text_future().await {
            Ok(Some(text)) => {
                let (added, _bad) = add_configs(&s, &text);
                let u = s.borrow();
                if added > 0 {
                    if !u.toggle.is_active() {
                        u.status.set_text("Disconnected");
                    }
                } else {
                    u.status.set_text("Disconnected");
                    u.connection_detail
                        .set_text("No supported configuration found");
                    u.empty_instruction.set_text(
                        "Paste a VLESS, VMess, Trojan, Shadowsocks or SSH configuration.",
                    );
                }
            }
            _ => s.borrow().status.set_text("Clipboard unavailable"),
        }
    });
}

fn import_dialog(s: &Rc<RefCell<Ui>>, parent: &gtk::ApplicationWindow) {
    let dialog = gtk::Dialog::builder()
        .title("Add Server")
        .transient_for(parent)
        .modal(true)
        .default_width(520)
        .default_height(360)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let add = dialog.add_button("Add Server", gtk::ResponseType::Accept);
    add.add_css_class("suggested-action");

    let content = dialog.content_area();
    content.add_css_class("dialog-content");
    content.set_spacing(10);
    content.append(&styled_label("Paste configuration", "dialog-title", 0.0));
    let supported = styled_label(
        "Supported: VLESS / Reality / VMess / Trojan / Shadowsocks / SSH",
        "dialog-description",
        0.0,
    );
    supported.set_wrap(true);
    content.append(&supported);
    let buffer = gtk::TextBuffer::new(None);
    let input = gtk::TextView::builder()
        .buffer(&buffer)
        .wrap_mode(gtk::WrapMode::Char)
        .monospace(true)
        .build();
    input.add_css_class("config-input");
    let input_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .min_content_height(170)
        .child(&input)
        .build();
    content.append(&input_scroll);
    let feedback = styled_label(
        "Multiple configurations can be added one per line.",
        "form-hint",
        0.0,
    );
    feedback.set_wrap(true);
    content.append(&feedback);

    {
        let buffer = buffer.clone();
        let clipboard = gdk::Display::default().unwrap().clipboard();
        glib::MainContext::default().spawn_local(async move {
            if let Ok(Some(text)) = clipboard.read_text_future().await {
                if !config::parse_many(&text).0.is_empty() {
                    buffer.set_text(&text);
                }
            }
        });
    }
    {
        let state = s.clone();
        let buffer = buffer.clone();
        let feedback = feedback.clone();
        dialog.connect_response(move |dialog, response| {
            if response != gtk::ResponseType::Accept {
                dialog.close();
                return;
            }
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            let (added, bad) = add_configs(&state, &text);
            if added == 0 {
                feedback.add_css_class("form-error");
                feedback.set_text(if bad.is_empty() {
                    "Paste at least one supported configuration."
                } else {
                    "The configuration could not be validated."
                });
            } else {
                if !state.borrow().toggle.is_active() {
                    state.borrow().status.set_text("Disconnected");
                }
                dialog.close();
            }
        });
    }
    dialog.present();
    input.grab_focus();
}

fn settings_dialog(parent: &gtk::ApplicationWindow) {
    let dialog = gtk::Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Settings")
        .default_width(440)
        .build();
    dialog.add_button("Close", gtk::ResponseType::Close);
    dialog.connect_response(|dialog, _| dialog.close());

    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.add_css_class("settings-content");
    content.set_margin_start(22);
    content.set_margin_end(22);
    content.set_margin_top(18);
    content.set_margin_bottom(8);

    let update_section = gtk::Box::new(gtk::Orientation::Vertical, 7);
    update_section.append(&styled_label("Software Update", "panel-title", 0.0));
    update_section.append(&styled_label(
        &format!("Installed version: {}", env!("CARGO_PKG_VERSION")),
        "panel-description",
        0.0,
    ));
    let update_status = styled_label(
        "Check GitHub Releases for a newer verified package.",
        "update-status",
        0.0,
    );
    update_status.set_wrap(true);
    update_section.append(&update_status);
    let update_button = gtk::Button::with_label("Check for Updates");
    update_button.add_css_class("primary-action");
    update_button.set_halign(gtk::Align::Start);
    update_section.append(&update_button);
    content.append(&update_section);

    let about_section = gtk::Box::new(gtk::Orientation::Vertical, 7);
    about_section.add_css_class("about-section");
    about_section.append(&styled_label("About V2Engine", "panel-title", 0.0));
    about_section.append(&styled_label(
        "Lightweight Linux proxy client powered by sing-box.",
        "panel-description",
        0.0,
    ));
    about_section.append(&styled_label("Ali Ranjbar Jelodar", "metric-value", 0.0));
    let github =
        gtk::LinkButton::with_label("https://github.com/RanjbarAli", "github.com/RanjbarAli");
    github.set_halign(gtk::Align::Start);
    github.add_css_class("author-link");
    about_section.append(&github);
    content.append(&about_section);
    dialog.content_area().append(&content);

    let pending_release = Rc::new(RefCell::new(None::<ReleaseInfo>));
    {
        let pending_release = pending_release.clone();
        let status = update_status.clone();
        let button = update_button.clone();
        update_button.connect_clicked(move |_| {
            button.set_sensitive(false);
            let install = pending_release.borrow().clone();
            status.set_text(if install.is_some() {
                "Downloading and verifying the update…"
            } else {
                "Checking GitHub Releases…"
            });
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || {
                let result = match install {
                    Some(release) => install_release(&release).map(|_| UpdateResult::Installed),
                    None => check_for_update(),
                };
                let _ = sender.send(result.map_err(|error| error.to_string()));
            });
            let pending_release = pending_release.clone();
            let status = status.clone();
            let button = button.clone();
            glib::timeout_add_local(Duration::from_millis(100), move || {
                match receiver.try_recv() {
                    Ok(Ok(UpdateResult::Available(release))) => {
                        status.set_text(&format!(
                            "Version {} is available. The package will be verified before installation.",
                            release.version
                        ));
                        button.set_label(&format!("Download & Install {}", release.version));
                        *pending_release.borrow_mut() = Some(release);
                        button.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                    Ok(Ok(UpdateResult::Current)) => {
                        status.set_text("V2Engine is up to date.");
                        button.set_label("Check Again");
                        button.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                    Ok(Ok(UpdateResult::Installed)) => {
                        status.set_text("Update installed. Restart V2Engine to use the new version.");
                        button.set_label("Installed");
                        *pending_release.borrow_mut() = None;
                        glib::ControlFlow::Break
                    }
                    Ok(Err(error)) => {
                        status.set_text(&format!("Update failed: {error}"));
                        button.set_label(if pending_release.borrow().is_some() {
                            "Try Installation Again"
                        } else {
                            "Try Again"
                        });
                        button.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        status.set_text("Update failed unexpectedly.");
                        button.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                }
            });
        });
    }
    dialog.present();
}

#[derive(Clone, Debug)]
struct ReleaseInfo {
    version: String,
    deb_url: String,
    checksum_url: String,
}

enum UpdateResult {
    Available(ReleaseInfo),
    Current,
    Installed,
}

const RELEASE_API: &str = "https://api.github.com/repos/RanjbarAli/V2Engine-linux/releases/latest";
const RELEASE_DOWNLOAD_PREFIX: &str =
    "https://github.com/RanjbarAli/V2Engine-linux/releases/download/";

fn check_for_update() -> anyhow::Result<UpdateResult> {
    let response = curl_bytes(RELEASE_API)?;
    let release: serde_json::Value = serde_json::from_slice(&response)?;
    if release.get("draft").and_then(|value| value.as_bool()) == Some(true)
        || release.get("prerelease").and_then(|value| value.as_bool()) == Some(true)
    {
        anyhow::bail!("the latest release is not a stable release");
    }
    let version = release
        .get("tag_name")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .strip_prefix('v')
        .unwrap_or_default();
    let remote =
        parse_version(version).ok_or_else(|| anyhow::anyhow!("invalid release version"))?;
    let current = parse_version(env!("CARGO_PKG_VERSION"))
        .ok_or_else(|| anyhow::anyhow!("invalid installed version"))?;
    if remote <= current {
        return Ok(UpdateResult::Current);
    }

    let deb_name = format!("V2Engine_{version}_amd64.deb");
    let checksum_name = format!("{deb_name}.sha256");
    let assets = release
        .get("assets")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("release assets are missing"))?;
    let asset_url = |name: &str| {
        assets.iter().find_map(|asset| {
            (asset.get("name")?.as_str()? == name).then(|| {
                asset
                    .get("browser_download_url")?
                    .as_str()
                    .map(str::to_owned)
            })?
        })
    };
    let deb_url =
        asset_url(&deb_name).ok_or_else(|| anyhow::anyhow!("update package is missing"))?;
    let checksum_url =
        asset_url(&checksum_name).ok_or_else(|| anyhow::anyhow!("update checksum is missing"))?;
    if !deb_url.starts_with(RELEASE_DOWNLOAD_PREFIX)
        || !checksum_url.starts_with(RELEASE_DOWNLOAD_PREFIX)
    {
        anyhow::bail!("release contains an untrusted download URL");
    }
    Ok(UpdateResult::Available(ReleaseInfo {
        version: version.to_owned(),
        deb_url,
        checksum_url,
    }))
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut pieces = version.split('.');
    let parsed = (
        pieces.next()?.parse().ok()?,
        pieces.next()?.parse().ok()?,
        pieces.next()?.parse().ok()?,
    );
    pieces.next().is_none().then_some(parsed)
}

fn curl_bytes(url: &str) -> anyhow::Result<Vec<u8>> {
    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "30",
            "--header",
            "Accept: application/vnd.github+json",
            "--header",
            "X-GitHub-Api-Version: 2022-11-28",
            "--user-agent",
            "V2Engine-Updater",
            url,
        ])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("could not reach GitHub Releases");
    }
    Ok(output.stdout)
}

fn install_release(release: &ReleaseInfo) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let expected = String::from_utf8(curl_bytes(&release.checksum_url)?)?
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("invalid release checksum");
    }

    let directory = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("cache directory is unavailable"))?
        .join("v2engine/updates");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let package = directory.join(format!("V2Engine_{}_amd64.deb", release.version));
    let partial = package.with_extension("deb.part");
    let _ = fs::remove_file(&partial);
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&partial)?;
    let download = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "300",
            "--user-agent",
            "V2Engine-Updater",
            "--output",
        ])
        .arg(&partial)
        .arg(&release.deb_url)
        .status()?;
    if !download.success() {
        let _ = fs::remove_file(&partial);
        anyhow::bail!("package download failed");
    }
    let digest = Command::new("sha256sum").arg(&partial).output()?;
    if !digest.status.success() {
        let _ = fs::remove_file(&partial);
        anyhow::bail!("could not verify the package");
    }
    let actual = String::from_utf8(digest.stdout)?
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if actual != expected {
        let _ = fs::remove_file(&partial);
        anyhow::bail!("package checksum mismatch");
    }
    fs::rename(&partial, &package)?;
    let installed = Command::new("pkexec")
        .arg("/usr/bin/apt-get")
        .arg("install")
        .arg("-y")
        .arg(&package)
        .status()?;
    if !installed.success() {
        anyhow::bail!("installation was cancelled or failed");
    }
    Ok(())
}
fn copy_selected(s: &Rc<RefCell<Ui>>) {
    let u = s.borrow();
    if let Some(x) = u
        .store
        .servers
        .iter()
        .find(|x| Some(&x.id) == u.store.selected.as_ref())
    {
        gdk::Display::default()
            .unwrap()
            .clipboard()
            .set_text(&x.uri);
    }
}
fn delete_selected(s: &Rc<RefCell<Ui>>) {
    let mut u = s.borrow_mut();
    if u.toggle.is_active() {
        u.connection_detail
            .set_text("Disconnect before deleting this server.");
        return;
    }
    let Some(id) = u.store.selected.take() else {
        return;
    };
    u.store.servers.retain(|x| x.id != id);
    u.store.selected = u.store.servers.first().map(|x| x.id.clone());
    let _ = config::save(&u.store);
    drop(u);
    refresh(s)
}

fn runtime_config(server: &Server, bypass: &[String]) -> anyhow::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let uid = unsafe { libc::getuid() };
    let dir = PathBuf::from(format!("/run/user/{uid}/v2engine"));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let path = dir.join("config.json");
    let mut f = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&path)?;
    serde_json::to_writer_pretty(&mut f, &config::singbox_config(server, bypass, true, None)?)?;
    f.sync_all()?;
    Ok(path)
}

fn connect_toggle(s: &Rc<RefCell<Ui>>, _w: &gtk::ApplicationWindow, on: bool) {
    let mut u = s.borrow_mut();
    if u.busy {
        return;
    }
    let args = if on {
        let Some(server) = u
            .store
            .servers
            .iter()
            .find(|x| Some(&x.id) == u.store.selected.as_ref())
        else {
            u.toggle.set_active(false);
            u.status.set_text("Disconnected");
            u.connection_detail
                .set_text("Add a server before connecting.");
            u.empty_instruction
                .set_text("Add a server before connecting.");
            u.server_panel.add_css_class("needs-attention");
            return;
        };
        match runtime_config(server, &u.store.bypass) {
            Ok(p) => vec!["start".into(), p.to_string_lossy().into_owned()],
            Err(e) => {
                u.toggle.set_active(false);
                u.status.set_text("Connection failed");
                u.connection_detail
                    .set_text(&format!("Invalid config: {e}"));
                return;
            }
        }
    } else {
        vec!["stop".into()]
    };
    u.busy = true;
    u.toggle.set_sensitive(false);
    u.status.set_text(if on {
        "Connecting…"
    } else {
        "Disconnecting…"
    });
    let (sender, receiver) = mpsc::channel();
    let transient = if on {
        args.get(1).map(PathBuf::from)
    } else {
        None
    };
    thread::spawn(move || {
        let result = Command::new("pkexec")
            .arg("/usr/lib/v2engine/v2engine-helper")
            .args(args)
            .output()
            .map(|o| {
                if o.status.success() {
                    Ok(())
                } else {
                    Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
                }
            })
            .unwrap_or_else(|e| Err(e.to_string()));
        if let Some(path) = transient {
            let _ = fs::remove_file(path);
        }
        let _ = sender.send(result);
    });
    let s = s.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        match receiver.try_recv() {
            Ok(result) => {
                let mut u = s.borrow_mut();
                u.busy = false;
                u.toggle.set_sensitive(true);
                let mut timer = None;
                match result {
                    Ok(()) => {
                        if on {
                            u.status.set_text("Connected");
                            u.status_dot.add_css_class("connected");
                            if let Some(server) = u
                                .store
                                .servers
                                .iter()
                                .find(|server| Some(&server.id) == u.store.selected.as_ref())
                            {
                                let protocol = protocol_label(server);
                                u.connection_detail
                                    .set_text(&format!("{} · {}", server.name, protocol));
                                u.protocol_value.set_text(&protocol);
                            }
                            let started = Instant::now();
                            u.connected_since = Some(started);
                            u.duration.set_text("00:00:00");
                            timer = Some(started);
                        } else {
                            u.status.set_text("Disconnected");
                            u.status_dot.remove_css_class("connected");
                            u.current_ip.set_text("—");
                            u.protocol_value.set_text("—");
                            u.upload.set_text("—");
                            u.download.set_text("—");
                            u.duration.set_text("—");
                            u.connected_since = None;
                            if let Some(server) = u
                                .store
                                .servers
                                .iter()
                                .find(|server| Some(&server.id) == u.store.selected.as_ref())
                            {
                                u.connection_detail.set_text(&format!(
                                    "{} · {}",
                                    server.name,
                                    protocol_label(server)
                                ));
                            } else {
                                u.connection_detail.set_text("No server selected");
                            }
                        }
                    }
                    Err(e) => {
                        u.status.set_text(if e.is_empty() {
                            "Disconnected"
                        } else {
                            "Connection failed"
                        });
                        u.status_dot.remove_css_class("connected");
                        u.connection_detail.set_text(if e.is_empty() {
                            "Connection cancelled"
                        } else {
                            "Check the selected server and try again."
                        });
                        if on {
                            u.toggle.set_active(false)
                        } else {
                            u.toggle.set_active(true)
                        }
                    }
                }
                drop(u);
                if let Some(started) = timer {
                    start_duration_timer(&s, started);
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(_) => glib::ControlFlow::Break,
        }
    });
}

fn start_duration_timer(s: &Rc<RefCell<Ui>>, started: Instant) {
    let state = s.clone();
    glib::timeout_add_local(Duration::from_secs(1), move || {
        let u = state.borrow();
        if u.connected_since != Some(started) {
            return glib::ControlFlow::Break;
        }
        let seconds = started.elapsed().as_secs();
        u.duration.set_text(&format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        ));
        glib::ControlFlow::Continue
    });
}

fn populate_domains(list: &gtk::ListBox, s: &Rc<RefCell<Ui>>) {
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
    let domains = s.borrow().store.bypass.clone();
    if domains.is_empty() {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        row.set_child(Some(&styled_label(
            "No direct sites added",
            "direct-empty",
            0.0,
        )));
        list.append(&row);
        return;
    }
    for domain in domains {
        let row = gtk::ListBoxRow::new();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.set_margin_top(6);
        line.set_margin_bottom(6);
        line.set_margin_start(8);
        line.set_margin_end(4);
        let label = gtk::Label::new(Some(&domain));
        label.set_hexpand(true);
        label.set_xalign(0.0);
        label.add_css_class("domain-name");
        let remove = gtk::Button::with_label("×");
        remove.add_css_class("domain-remove");
        remove.set_tooltip_text(Some("Remove domain"));
        let state = s.clone();
        let list_copy = list.clone();
        let domain_copy = domain.clone();
        remove.connect_clicked(move |_| {
            state
                .borrow_mut()
                .store
                .bypass
                .retain(|x| x != &domain_copy);
            let _ = config::save(&state.borrow().store);
            populate_domains(&list_copy, &state);
        });
        line.append(&label);
        line.append(&remove);
        row.set_child(Some(&line));
        list.append(&row);
    }
}

fn domain_dialog(s: &Rc<RefCell<Ui>>, parent: &gtk::ApplicationWindow) {
    let dialog = gtk::Dialog::builder()
        .title("Add Direct Site")
        .transient_for(parent)
        .modal(true)
        .default_width(420)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let add = dialog.add_button("Add Domain", gtk::ResponseType::Accept);
    add.add_css_class("suggested-action");
    let content = dialog.content_area();
    content.add_css_class("dialog-content");
    content.set_spacing(10);
    let hint = styled_label(
        "The domain and all of its subdomains will use your normal connection.",
        "dialog-description",
        0.0,
    );
    hint.set_wrap(true);
    content.append(&hint);
    let entry = gtk::Entry::builder()
        .placeholder_text("example.com")
        .build();
    content.append(&entry);
    let feedback = styled_label(
        "Enter a domain without a path or protocol.",
        "form-hint",
        0.0,
    );
    content.append(&feedback);
    {
        let state = s.clone();
        let entry = entry.clone();
        let feedback = feedback.clone();
        dialog.connect_response(move |dialog, response| {
            if response != gtk::ResponseType::Accept {
                dialog.close();
                return;
            }
            let domain = entry
                .text()
                .trim()
                .trim_start_matches("*.")
                .trim_start_matches('.')
                .to_ascii_lowercase();
            if !config::valid_domain(&domain) {
                feedback.add_css_class("form-error");
                feedback.set_text("Enter a valid domain, for example google.com.");
                return;
            }
            if !state.borrow().store.bypass.contains(&domain) {
                state.borrow_mut().store.bypass.push(domain);
                let _ = config::save(&state.borrow().store);
                let list = state.borrow().direct_list.clone();
                populate_domains(&list, &state);
            }
            dialog.close();
        });
    }
    dialog.present();
    entry.grab_focus();
}

fn singbox_path() -> PathBuf {
    let installed = PathBuf::from("/usr/lib/v2engine/sing-box");
    if installed.exists() {
        installed
    } else {
        PathBuf::from("vendor/sing-box")
    }
}
fn system_connected() -> bool {
    let Ok(pid) = fs::read_to_string("/run/v2engine/sing-box.pid") else {
        return false;
    };
    let Ok(pid) = pid.trim().parse::<u32>() else {
        return false;
    };
    fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .is_some_and(|p| p == std::path::Path::new("/usr/lib/v2engine/sing-box"))
}
fn free_port() -> Option<u16> {
    TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()
        .map(|a| a.port())
}
fn test_one(server: &Server) -> String {
    let Some(port) = free_port() else {
        return "Failed".into();
    };
    let Ok(cfg) = config::singbox_config(server, &[], false, Some(port)) else {
        return "Failed".into();
    };
    let dir = std::env::temp_dir().join(format!("v2engine-test-{}-{}", std::process::id(), port));
    if fs::create_dir(&dir).is_err() {
        return "Failed".into();
    }
    let path = dir.join("config.json");
    if fs::write(&path, serde_json::to_vec(&cfg).unwrap()).is_err() {
        return "Failed".into();
    }
    let mut child = match Command::new(singbox_path())
        .args(["run", "-c"])
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return "Failed".into(),
    };
    thread::sleep(Duration::from_millis(350));
    let start = Instant::now();
    let result = (|| -> std::io::Result<()> {
        let mut sock = TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_secs(6),
        )?;
        sock.set_read_timeout(Some(Duration::from_secs(7)))?;
        sock.write_all(&[5, 1, 0])?;
        let mut b = [0u8; 2];
        sock.read_exact(&mut b)?;
        if b != [5, 0] {
            return Err(std::io::ErrorKind::Other.into());
        }
        let host = b"www.gstatic.com";
        let mut req = vec![5, 1, 0, 3, host.len() as u8];
        req.extend(host);
        req.extend(443u16.to_be_bytes());
        sock.write_all(&req)?;
        let mut reply = [0u8; 10];
        sock.read_exact(&mut reply[..4])?;
        if reply[1] != 0 {
            return Err(std::io::ErrorKind::ConnectionRefused.into());
        }
        Ok(())
    })();
    let elapsed = start.elapsed().as_millis();
    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(path);
    let _ = fs::remove_dir(dir);
    match result {
        Ok(()) => format!("{elapsed} ms"),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            "Timeout".into()
        }
        Err(_) => "Failed".into(),
    }
}

fn run_tests(s: &Rc<RefCell<Ui>>, button: &gtk::Button) {
    let servers = s.borrow().store.servers.clone();
    if servers.is_empty() {
        return;
    }
    for server in &mut s.borrow_mut().store.servers {
        server.latency = Some("Testing…".into());
    }
    refresh(s);
    button.set_sensitive(false);
    button.set_label("Testing…");
    let (queue_tx, queue_rx) = mpsc::channel();
    let work = Arc::new(Mutex::new(servers.into_iter()));
    for _ in 0..4 {
        let work = work.clone();
        let tx = queue_tx.clone();
        thread::spawn(move || loop {
            let next = work.lock().unwrap().next();
            let Some(server) = next else { break };
            let result = test_one(&server);
            let _ = tx.send((server.id, result));
        });
    }
    drop(queue_tx);
    let s = s.clone();
    let button = button.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let mut changed = false;
        loop {
            match queue_rx.try_recv() {
                Ok((id, result)) => {
                    if let Some(x) = s.borrow_mut().store.servers.iter_mut().find(|x| x.id == id) {
                        x.latency = Some(result);
                        changed = true
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if changed {
                        refresh(&s)
                    }
                    button.set_sensitive(true);
                    button.set_label("Test All");
                    return glib::ControlFlow::Break;
                }
            }
        }
        if changed {
            refresh(&s)
        }
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::{config, parse_version};
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::{fs, process::Command};
    #[test]
    fn rejects_unknown() {
        assert!(config::parse("https://example.com").is_err())
    }
    #[test]
    fn domains() {
        assert!(config::valid_domain("google.com"));
        assert!(!config::valid_domain("bad domain"))
    }
    #[test]
    fn parses_vless() {
        assert!(config::parse(
            "vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?security=tls#Demo"
        )
        .is_ok())
    }

    #[test]
    fn parses_release_versions_strictly() {
        assert_eq!(parse_version("2.3.4"), Some((2, 3, 4)));
        assert_eq!(parse_version("1.1"), None);
        assert_eq!(parse_version("2.3.4-beta"), None);
        assert_eq!(parse_version("2.3.4.5"), None);
    }

    #[test]
    fn sing_box_accepts_every_supported_outbound() {
        let vmess = format!(
            "vmess://{}",
            STANDARD.encode(r#"{"v":"2","ps":"VMess","add":"example.com","port":"443","id":"123e4567-e89b-12d3-a456-426614174000","scy":"auto","net":"tcp","tls":"tls","sni":"example.com"}"#)
        );
        let ss_auth = STANDARD.encode("aes-128-gcm:test-password");
        let fixtures = [
            "vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?security=tls&sni=example.com#VLESS".to_string(),
            "vless://123e4567-e89b-12d3-a456-426614174000@example.com:443?security=reality&sni=example.com&pbk=cfAKRnsT7MiYOmA8wz0PJcEYD1mTRsoO-VNyv6CAEAc&sid=0123456789abcdef&fp=chrome#Reality".to_string(),
            vmess,
            "trojan://test-password@example.com:443?security=tls&sni=example.com#Trojan".to_string(),
            format!("ss://{ss_auth}@example.com:8388#Shadowsocks"),
            "ssh://demo:test-password@example.com:22#SSH".to_string(),
        ];
        for (index, fixture) in fixtures.iter().enumerate() {
            let server = config::parse(fixture).unwrap();
            let generated =
                config::singbox_config(&server, &["google.com".into()], index == 0, Some(19090))
                    .unwrap();
            let path = std::env::temp_dir().join(format!(
                "v2engine-schema-{}-{index}.json",
                std::process::id()
            ));
            fs::write(&path, serde_json::to_vec(&generated).unwrap()).unwrap();
            let status = Command::new("vendor/sing-box")
                .args(["check", "-c"])
                .arg(&path)
                .status()
                .unwrap();
            let _ = fs::remove_file(path);
            assert!(status.success(), "sing-box rejected {}", server.protocol);
        }
    }
}
