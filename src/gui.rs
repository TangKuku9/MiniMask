//! Embedded graphical client for MiniMask.
//!
//! When the executable is double-clicked (no CLI args, not launched from an
//! existing console), `main` calls [`run`] which spins up this eframe/egui
//! application. It offers a friendly, fully-localized UI to configure the
//! connection, start/stop the tunnel client, watch live logs, and persist the
//! last-used settings.

use crate::client::{self, ClientArgs, ClientEvent, ConnState, LogLevel};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::mpsc as std_mpsc;
use std::time::Instant;

/// Maximum number of log lines kept in memory.
const MAX_LOG_LINES: usize = 2000;

/// Launch the GUI. Blocks until the window is closed.
pub fn run() -> anyhow::Result<()> {
    let icon = load_icon();
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([760.0, 620.0])
        .with_min_inner_size([620.0, 480.0])
        .with_title("MiniMask 客户端")
        .with_icon(std::sync::Arc::new(icon));

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "MiniMask 客户端",
        options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            Ok(Box::new(App::new()) as Box<dyn eframe::App>)
        }),
    )
    .map_err(|e| anyhow::anyhow!("GUI 启动失败：{e}"))
}

/// Build a small procedurally-generated icon so the window/taskbar isn't blank.
fn load_icon() -> egui::IconData {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            // A simple diagonal gradient in the brand indigo/violet range.
            let fx = x as f32 / size as f32;
            let fy = y as f32 / size as f32;
            let r = (80.0 + 90.0 * fx) as u8;
            let g = (70.0 + 40.0 * fy) as u8;
            let b = (200.0 + 40.0 * (1.0 - fx)) as u8;
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    egui::IconData { rgba, width: size, height: size }
}

/// Register a font that can render CJK glyphs. We try a few common Windows
/// system fonts; if none are found, egui's default fonts are used (Latin only).
fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",     // 微软雅黑
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simhei.ttf",   // 黑体
        r"C:\Windows\Fonts\simsun.ttc",   // 宋体
        r"C:\Windows\Fonts\Deng.ttf",     // 等线
    ];

    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("cjk".to_owned(), egui::FontData::from_owned(bytes).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "cjk".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

// ---------------------------------------------------------------------------
// Persisted settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    server: String,
    id: String,
    token: String,
    tls: bool,
    server_name: String,
    /// Path to the pinned CA certificate (PEM). Used to verify the server's
    /// TLS certificate. Defaults to `./data/ca.pem` (distributed from server).
    #[serde(default = "default_ca_path")]
    ca_path: String,
    /// Skip TLS certificate verification. INSECURE — only for local debugging.
    #[serde(default)]
    insecure_skip_verify: bool,
    dark_mode: bool,
}

fn default_ca_path() -> String {
    "./data/ca.pem".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: "127.0.0.1:7443".to_string(),
            id: String::new(),
            token: String::new(),
            tls: true,
            server_name: "localhost".to_string(),
            ca_path: "./data/ca.pem".to_string(),
            insecure_skip_verify: false,
            dark_mode: true,
        }
    }
}

impl Settings {
    fn config_path() -> Option<std::path::PathBuf> {
        // Store next to the executable's working dir under ./data for parity
        // with the server, falling back to the current dir.
        let dir = std::path::PathBuf::from("data");
        std::fs::create_dir_all(&dir).ok();
        Some(dir.join("client_gui.json"))
    }

    fn load() -> Self {
        if let Some(path) = Self::config_path() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(s) = serde_json::from_str::<Settings>(&text) {
                    return s;
                }
            }
        }
        Settings::default()
    }

    fn save(&self) {
        if let Some(path) = Self::config_path() {
            if let Ok(text) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(path, text);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Log entry
// ---------------------------------------------------------------------------

struct LogLine {
    level: LogLevel,
    time: String,
    message: String,
}

// ---------------------------------------------------------------------------
// Running tunnel handle
// ---------------------------------------------------------------------------

/// Owns the background tokio runtime + task while the tunnel is running.
struct Running {
    cancel: client::CancelToken,
    events: std_mpsc::Receiver<ClientEvent>,
    // Keep the runtime alive; dropping it shuts everything down.
    _runtime: tokio::runtime::Runtime,
    started_at: Instant,
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

struct App {
    settings: Settings,
    show_token: bool,
    state: Option<ConnState>,
    running: Option<Running>,
    logs: VecDeque<LogLine>,
    auto_scroll: bool,
    reconnects: u32,
}

impl App {
    fn new() -> Self {
        let settings = Settings::load();
        Self {
            settings,
            show_token: false,
            state: None,
            running: None,
            logs: VecDeque::new(),
            auto_scroll: true,
            reconnects: 0,
        }
    }

    fn is_running(&self) -> bool {
        self.running.is_some()
    }

    fn push_log(&mut self, level: LogLevel, message: String) {
        let time = current_time_string();
        if self.logs.len() >= MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(LogLine { level, time, message });
    }

    fn validate(&self) -> Result<(), String> {
        if self.settings.server.trim().is_empty() {
            return Err("请填写服务器地址".into());
        }
        if !self.settings.server.contains(':') {
            return Err("服务器地址需包含端口，如 127.0.0.1:7443".into());
        }
        if self.settings.id.trim().is_empty() {
            return Err("请填写客户端 ID".into());
        }
        if self.settings.token.trim().is_empty() {
            return Err("请填写 Token".into());
        }
        Ok(())
    }

    fn start(&mut self) {
        if self.is_running() {
            return;
        }
        if let Err(e) = self.validate() {
            self.push_log(LogLevel::Error, e);
            return;
        }

        self.settings.save();
        self.reconnects = 0;

        let args = ClientArgs {
            server: self.settings.server.trim().to_string(),
            id: self.settings.id.trim().to_string(),
            token: self.settings.token.trim().to_string(),
            tls: self.settings.tls,
            server_name: self.settings.server_name.trim().to_string(),
            ca_path: self.settings.ca_path.trim().to_string(),
            insecure_skip_verify: self.settings.insecure_skip_verify,
        };

        // Bridge tokio's async channel to a std channel the UI thread can poll.
        let (std_tx, std_rx) = std_mpsc::channel::<ClientEvent>();

        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                self.push_log(LogLevel::Error, format!("无法创建运行时：{e}"));
                return;
            }
        };

        let cancel = client::CancelToken::new();
        let cancel_task = cancel.clone();

        // The event sink pushes directly into the std channel the UI polls.
        let sink = client::EventSink::channel(std_tx);
        runtime.spawn(async move {
            let _ = client::run_supervised(args, sink, cancel_task).await;
        });

        self.running = Some(Running {
            cancel,
            events: std_rx,
            _runtime: runtime,
            started_at: Instant::now(),
        });
        self.state = Some(ConnState::Connecting);
    }

    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.cancel.cancel();
            // P2-13: shut the runtime down with a bounded timeout instead of
            // plain `drop`. A bare `drop` waits forever for all spawned tasks
            // to finish, which can leak helper threads when the user toggles
            // connect/disconnect repeatedly. `shutdown_timeout` drains pending
            // tasks for at most 1s then forcibly cancels the rest.
            //
            // Still offloaded to a helper thread so the UI never blocks.
            std::thread::spawn(move || {
                let Running { _runtime, .. } = running;
                _runtime.shutdown_timeout(std::time::Duration::from_secs(1));
            });
        }
        self.state = None;
        self.push_log(LogLevel::Info, "已断开连接".to_string());
    }

    /// Drain events from the running tunnel into the log/state.
    fn pump_events(&mut self) {
        // Collect first to avoid borrowing self mutably while iterating.
        let mut drained: Vec<ClientEvent> = Vec::new();
        if let Some(running) = &self.running {
            while let Ok(ev) = running.events.try_recv() {
                drained.push(ev);
            }
        }
        for ev in drained {
            match ev {
                ClientEvent::Log { level, message } => self.push_log(level, message),
                ClientEvent::State(s) => {
                    if s == ConnState::Reconnecting {
                        self.reconnects += 1;
                    }
                    self.state = Some(s);
                }
            }
        }
    }
}

fn current_time_string() -> String {
    use chrono::Local;
    Local::now().format("%H:%M:%S").to_string()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply theme.
        if self.settings.dark_mode {
            ctx.set_visuals(brand_dark_visuals());
        } else {
            ctx.set_visuals(brand_light_visuals());
        }

        self.pump_events();
        // Keep repainting while running so logs stream smoothly.
        if self.is_running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        self.top_bar(ctx);
        self.central(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.settings.save();
        if let Some(running) = &self.running {
            running.cancel.cancel();
        }
    }
}

impl App {
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("🦀 MiniMask 客户端").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.settings.dark_mode { "🌙 深色" } else { "☀ 浅色" };
                    if ui.button(label).clicked() {
                        self.settings.dark_mode = !self.settings.dark_mode;
                        self.settings.save();
                    }
                });
            });
            ui.add_space(6.0);
        });
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.status_card(ui);
            ui.add_space(10.0);
            self.config_card(ui);
            ui.add_space(10.0);
            self.log_card(ui);
        });
    }

    fn status_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style())
            .fill(ui.visuals().faint_bg_color)
            .rounding(10.0)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (text, color) = match (self.is_running(), self.state) {
                        (false, _) => ("● 未连接", egui::Color32::from_rgb(150, 150, 150)),
                        (true, Some(ConnState::Connecting)) => {
                            ("● 连接中", egui::Color32::from_rgb(230, 180, 60))
                        }
                        (true, Some(ConnState::Connected)) => {
                            ("● 已连接", egui::Color32::from_rgb(80, 200, 120))
                        }
                        (true, Some(ConnState::Reconnecting)) => {
                            ("● 重连中", egui::Color32::from_rgb(230, 140, 60))
                        }
                        (true, None) => ("● 启动中", egui::Color32::from_rgb(230, 180, 60)),
                    };
                    ui.label(egui::RichText::new(text).color(color).size(18.0).strong());

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(running) = &self.running {
                            let secs = running.started_at.elapsed().as_secs();
                            let h = secs / 3600;
                            let m = (secs % 3600) / 60;
                            let s = secs % 60;
                            ui.label(
                                egui::RichText::new(format!("运行时长 {:02}:{:02}:{:02}", h, m, s))
                                    .monospace(),
                            );
                            ui.separator();
                            ui.label(format!("重连次数 {}", self.reconnects));
                        }
                    });
                });
            });
    }

    fn config_card(&mut self, ui: &mut egui::Ui) {
        let running = self.is_running();
        egui::Frame::group(ui.style())
            .rounding(10.0)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("连接配置").strong().size(15.0));
                ui.add_space(8.0);

                egui::Grid::new("cfg_grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .min_col_width(90.0)
                    .show(ui, |ui| {
                        ui.label("服务器地址");
                        ui.add_enabled(
                            !running,
                            egui::TextEdit::singleline(&mut self.settings.server)
                                .hint_text("127.0.0.1:7443")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();

                        ui.label("客户端 ID");
                        ui.add_enabled(
                            !running,
                            egui::TextEdit::singleline(&mut self.settings.id)
                                .hint_text("在 Web 后台创建客户端后获得")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();

                        ui.label("Token");
                        ui.horizontal(|ui| {
                            ui.add_enabled(
                                !running,
                                egui::TextEdit::singleline(&mut self.settings.token)
                                    .password(!self.show_token)
                                    .hint_text("创建客户端时一次性下发")
                                    .desired_width(ui.available_width() - 60.0),
                            );
                            let eye = if self.show_token { "隐藏" } else { "显示" };
                            if ui.button(eye).clicked() {
                                self.show_token = !self.show_token;
                            }
                        });
                        ui.end_row();

                        ui.label("启用 TLS");
                        ui.add_enabled(
                            !running,
                            egui::Checkbox::new(&mut self.settings.tls, "使用 TLS 加密连接"),
                        );
                        ui.end_row();

                        ui.label("Server Name");
                        ui.add_enabled(
                            !running && self.settings.tls,
                            egui::TextEdit::singleline(&mut self.settings.server_name)
                                .hint_text("localhost（需与服务端证书 SAN 匹配）")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();

                        ui.label("CA 证书路径");
                        ui.add_enabled(
                            !running && self.settings.tls && !self.settings.insecure_skip_verify,
                            egui::TextEdit::singleline(&mut self.settings.ca_path)
                                .hint_text("data/ca.pem（从服务端拷贝）")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();

                        ui.label("跳过证书校验");
                        ui.add_enabled(
                            !running && self.settings.tls,
                            egui::Checkbox::new(
                                &mut self.settings.insecure_skip_verify,
                                "禁用 TLS 证书校验（仅本地调试，不安全）",
                            ),
                        );
                        ui.end_row();
                    });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if !running {
                        let connect = egui::Button::new(
                            egui::RichText::new("🔗 连接").size(15.0).strong(),
                        )
                        .min_size(egui::vec2(120.0, 34.0))
                        .fill(egui::Color32::from_rgb(99, 102, 241));
                        if ui.add(connect).clicked() {
                            self.start();
                        }
                    } else {
                        let disconnect = egui::Button::new(
                            egui::RichText::new("⛔ 断开").size(15.0).strong(),
                        )
                        .min_size(egui::vec2(120.0, 34.0))
                        .fill(egui::Color32::from_rgb(220, 80, 80));
                        if ui.add(disconnect).clicked() {
                            self.stop();
                        }
                    }

                    if ui
                        .add(egui::Button::new("💾 保存配置").min_size(egui::vec2(110.0, 34.0)))
                        .clicked()
                    {
                        self.settings.save();
                        self.push_log(LogLevel::Info, "配置已保存".to_string());
                    }
                });
            });
    }

    fn log_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style())
            .rounding(10.0)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("运行日志").strong().size(15.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("🗑 清空").clicked() {
                            self.logs.clear();
                        }
                        ui.checkbox(&mut self.auto_scroll, "自动滚动");
                    });
                });
                ui.add_space(6.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(self.auto_scroll)
                    .max_height(ui.available_height())
                    .show(ui, |ui| {
                        for line in &self.logs {
                            let color = match line.level {
                                LogLevel::Info => ui.visuals().text_color(),
                                LogLevel::Warn => egui::Color32::from_rgb(230, 170, 50),
                                LogLevel::Error => egui::Color32::from_rgb(230, 90, 90),
                            };
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("[{}]", line.time))
                                        .monospace()
                                        .weak(),
                                );
                                ui.label(egui::RichText::new(&line.message).color(color).monospace());
                            });
                        }
                        if self.logs.is_empty() {
                            ui.weak("暂无日志。填写配置后点击「连接」开始。");
                        }
                    });
            });
    }
}

// ---------------------------------------------------------------------------
// Theming
// ---------------------------------------------------------------------------

fn brand_dark_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = egui::Color32::from_rgb(24, 25, 34);
    v.window_fill = egui::Color32::from_rgb(24, 25, 34);
    v.faint_bg_color = egui::Color32::from_rgb(34, 36, 48);
    v.extreme_bg_color = egui::Color32::from_rgb(18, 19, 26);
    v.hyperlink_color = egui::Color32::from_rgb(129, 140, 248);
    v.selection.bg_fill = egui::Color32::from_rgb(99, 102, 241);
    v
}

fn brand_light_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    v.panel_fill = egui::Color32::from_rgb(246, 247, 251);
    v.faint_bg_color = egui::Color32::from_rgb(236, 238, 245);
    v.hyperlink_color = egui::Color32::from_rgb(79, 70, 229);
    v.selection.bg_fill = egui::Color32::from_rgb(129, 140, 248);
    v
}