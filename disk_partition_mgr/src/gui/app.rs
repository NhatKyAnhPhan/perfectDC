//! Giao diện chính của PerfectDS.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, RichText, Sense,
    Stroke, StrokeKind, Vec2,
};

use crate::gui::theme;
use crate::model::{self, DiskDto, Gap, MIB, human_bytes};
use crate::ops::{self, Action, Fs};
use crate::sys;

// ---------------------------------------------------------------------------
// Khởi động
// ---------------------------------------------------------------------------

pub fn run() -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1220.0, 820.0])
            .with_min_inner_size([980.0, 640.0])
            .with_title("PerfectDS — Trình quản lý & chia ổ đĩa")
            .with_icon(egui::IconData {
                rgba: include_bytes!("../../assets/icon64.rgba").to_vec(),
                width: 64,
                height: 64,
            }),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "PerfectDS",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Trạng thái
// ---------------------------------------------------------------------------

struct Busy {
    rx: Receiver<Msg>,
    label: String,
    started: Instant,
}

enum Msg {
    Disks(Result<Vec<DiskDto>, String>),
    Action {
        description: String,
        result: Result<String, String>,
    },
}

struct Status {
    text: String,
    ok: bool,
    at: Instant,
}

struct LogEntry {
    time: String,
    text: String,
    ok: bool,
}

struct CreateForm {
    disk: u32,
    offset: u64,
    max: u64,
    size_mib: u64,
    fs: Fs,
    label: String,
    letter: Option<char>,
}

struct SplitForm {
    disk: u32,
    partition: model::PartitionDto,
    max: u64,
    size_mib: u64,
    fs: Fs,
    label: String,
    letter: Option<char>,
}

struct ExtendForm {
    disk: u32,
    partition: model::PartitionDto,
    max: u64,
    size_mib: u64,
}

struct FormatForm {
    disk: u32,
    partition: model::PartitionDto,
    fs: Fs,
    label: String,
    full: bool,
    input: String,
}

struct ConfirmForm {
    action: Action,
    keyword: &'static str,
    input: String,
    warnings: Vec<String>,
}

struct LetterForm {
    disk: u32,
    partition: model::PartitionDto,
    letter: char,
    letters: Vec<char>,
}

struct ResultInfo {
    title: String,
    ok: bool,
    body: String,
}

enum Dialog {
    None,
    Create(CreateForm),
    Split(SplitForm),
    Extend(ExtendForm),
    Format(FormatForm),
    Confirm(ConfirmForm),
    Letter(LetterForm),
    About,
    Result(ResultInfo),
}

enum Outcome {
    None,
    Close,
    Start(Action),
    OpenLog,
    Copy(String),
}

pub struct App {
    disks: Vec<DiskDto>,
    selected_disk: Option<u32>,
    selected_part: Option<u32>,
    admin: bool,
    busy: Option<Busy>,
    dialog: Dialog,
    logs: Vec<LogEntry>,
    show_logs: bool,
    status: Option<Status>,
    first_load: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let mut app = Self {
            disks: Vec::new(),
            selected_disk: None,
            selected_part: None,
            admin: sys::is_admin()
                || (cfg!(debug_assertions) && std::env::var_os("PERFECTDS_UI_PREVIEW").is_some()),
            busy: None,
            dialog: Dialog::None,
            logs: Vec::new(),
            show_logs: false,
            status: None,
            first_load: true,
        };
        app.push_log(true, "PerfectDS khởi động.");
        app.start_load(&cc.egui_ctx);
        app
    }

    fn selected_disk_dto(&self) -> Option<&DiskDto> {
        self.selected_disk
            .and_then(|n| self.disks.iter().find(|d| d.number == n))
    }

    fn selected_partition(&self) -> Option<model::PartitionDto> {
        let d = self.selected_disk_dto()?;
        self.selected_part.and_then(|n| d.partition(n).cloned())
    }

    fn set_status(&mut self, ok: bool, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            ok,
            at: Instant::now(),
        });
    }

    fn push_log(&mut self, ok: bool, text: &str) {
        let full = sys::timestamp();
        let time = full.split(' ').nth(1).unwrap_or(full.as_str()).to_string();
        self.logs.push(LogEntry {
            time,
            text: text.to_string(),
            ok,
        });
        if self.logs.len() > 500 {
            self.logs.remove(0);
        }
    }

    fn start_load(&mut self, ctx: &egui::Context) {
        if self.busy.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        thread::spawn(move || {
            let r = ops::fetch_disks();
            let _ = tx.send(Msg::Disks(r));
            ctx2.request_repaint();
        });
        self.busy = Some(Busy {
            rx,
            label: "Đang đọc cấu hình ổ đĩa…".to_string(),
            started: Instant::now(),
        });
    }

    fn start_action(&mut self, ctx: &egui::Context, action: Action) {
        if let Err(e) = action.validate(&self.disks) {
            self.push_log(false, &e);
            self.set_status(false, "Thao tác không hợp lệ.");
            self.dialog = Dialog::Result(ResultInfo {
                title: "Không thể thực hiện".into(),
                ok: false,
                body: e,
            });
            return;
        }
        let description = action.description();
        let title = action.title().to_string();
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        let act = action.clone();
        let desc2 = description.clone();
        sys::append_log("REQUEST", &description);
        thread::spawn(move || {
            let result = act.run();
            let _ = tx.send(Msg::Action {
                description: desc2,
                result,
            });
            ctx2.request_repaint();
        });
        self.busy = Some(Busy {
            rx,
            label: format!("Đang thực hiện: {title}…"),
            started: Instant::now(),
        });
        self.dialog = Dialog::None;
    }

    fn elevate(&mut self, ctx: &egui::Context) {
        match sys::relaunch_as_admin() {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => {
                self.push_log(false, &e);
                self.set_status(false, "Không thể nâng quyền.");
                self.dialog = Dialog::Result(ResultInfo {
                    title: "Cần quyền Administrator".into(),
                    ok: false,
                    body: e,
                });
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(busy) = self.busy.take() else {
            return;
        };
        match busy.rx.try_recv() {
            Ok(msg) => {
                self.busy = None;
                match msg {
                    Msg::Disks(Ok(list)) => {
                        let n = list.len();
                        self.disks = list;
                        self.fix_selection();
                        self.first_load = false;
                        self.push_log(true, &format!("Đọc cấu hình {n} ổ đĩa thành công."));
                        self.set_status(true, format!("Đã cập nhật {n} ổ đĩa."));
                    }
                    Msg::Disks(Err(e)) => {
                        self.first_load = false;
                        self.push_log(false, &e);
                        self.set_status(false, "Không đọc được cấu hình ổ đĩa.");
                        self.dialog = Dialog::Result(ResultInfo {
                            title: "Không đọc được ổ đĩa".into(),
                            ok: false,
                            body: e,
                        });
                    }
                    Msg::Action {
                        description,
                        result,
                    } => {
                        match result {
                            Ok(msg) => {
                                self.push_log(true, &msg);
                                self.set_status(true, &description);
                                self.dialog = Dialog::Result(ResultInfo {
                                    title: "Hoàn tất".into(),
                                    ok: true,
                                    body: msg,
                                });
                            }
                            Err(e) => {
                                self.push_log(false, &format!("{description}: {e}"));
                                self.set_status(false, "Thao tác thất bại.");
                                self.dialog = Dialog::Result(ResultInfo {
                                    title: "Thao tác thất bại".into(),
                                    ok: false,
                                    body: e,
                                });
                            }
                        }
                        self.start_load(ctx);
                    }
                }
            }
            Err(TryRecvError::Empty) => {
                self.busy = Some(busy);
                ctx.request_repaint_after(Duration::from_millis(120));
            }
            Err(TryRecvError::Disconnected) => {
                self.busy = None;
                self.first_load = false;
                self.push_log(false, "Tiến trình nền bị ngắt.");
                self.set_status(false, "Tiến trình nền bị ngắt.");
            }
        }
    }

    fn fix_selection(&mut self) {
        let sel = self
            .selected_disk
            .filter(|n| self.disks.iter().any(|d| d.number == *n))
            .or_else(|| self.disks.iter().find(|d| !d.protected()).map(|d| d.number))
            .or_else(|| self.disks.first().map(|d| d.number));
        self.selected_disk = sel;
        let part_ok = match (self.selected_disk, self.selected_part) {
            (Some(dn), Some(pn)) => self
                .disks
                .iter()
                .find(|d| d.number == dn)
                .map(|d| d.partitions.iter().any(|p| p.number == pn))
                .unwrap_or(false),
            _ => false,
        };
        if !part_ok {
            self.selected_part = None;
        }
    }

    // -- Mở các hộp thoại ---------------------------------------------------

    fn open_create(&mut self, disk: &DiskDto, gap: Gap) {
        let max = model::align_down(gap.size, MIB).max(16 * MIB);
        self.dialog = Dialog::Create(CreateForm {
            disk: disk.number,
            offset: gap.start,
            max,
            size_mib: max / MIB,
            fs: Fs::Ntfs,
            label: String::new(),
            letter: ops::suggest_letter(&self.disks),
        });
    }

    fn open_split(&mut self, p: &model::PartitionDto) {
        let max = ops::max_split_size(p);
        let size = (max / 2 / MIB * MIB).max(256 * MIB).min(max);
        self.dialog = Dialog::Split(SplitForm {
            disk: p_disk(self, p),
            partition: p.clone(),
            max,
            size_mib: size / MIB,
            fs: Fs::Ntfs,
            label: String::new(),
            letter: ops::suggest_letter(&self.disks),
        });
    }

    fn open_extend(&mut self, p: &model::PartitionDto) {
        self.dialog = Dialog::Extend(ExtendForm {
            disk: p_disk(self, p),
            partition: p.clone(),
            max: p.max_size,
            size_mib: p.max_size / MIB,
        });
    }

    fn open_format(&mut self, p: &model::PartitionDto) {
        let fs = match p.fs.as_str() {
            "exFAT" => Fs::ExFat,
            "FAT32" => Fs::Fat32,
            _ => Fs::Ntfs,
        };
        self.dialog = Dialog::Format(FormatForm {
            disk: p_disk(self, p),
            partition: p.clone(),
            fs,
            label: p.label.clone(),
            full: false,
            input: String::new(),
        });
    }

    fn open_delete(&mut self, p: &model::PartitionDto) {
        let action = Action::Delete {
            disk: p_disk(self, p),
            partition: p.number,
        };
        let mut warnings = vec![
            "Toàn bộ dữ liệu trên phân vùng này sẽ bị xoá vĩnh viễn.".to_string(),
            "Không thể hoàn tác. Hãy sao lưu dữ liệu quan trọng trước.".to_string(),
        ];
        if p.bitlocker_on() {
            warnings.push(
                "Phân vùng đang được BitLocker bảo vệ — dữ liệu sẽ mất và không thể khôi phục."
                    .to_string(),
            );
        }
        self.dialog = Dialog::Confirm(ConfirmForm {
            action,
            keyword: "DELETE",
            input: String::new(),
            warnings,
        });
    }

    fn open_letter(&mut self, p: &model::PartitionDto) {
        let letters = model::free_letters(&self.disks);
        let letter = p
            .letter_opt()
            .or_else(|| letters.first().copied())
            .unwrap_or('D');
        self.dialog = Dialog::Letter(LetterForm {
            disk: p_disk(self, p),
            partition: p.clone(),
            letter,
            letters,
        });
    }

    fn open_initialize(&mut self, disk: &DiskDto) {
        self.dialog = Dialog::Confirm(ConfirmForm {
            action: Action::Initialize { disk: disk.number },
            keyword: "INIT",
            input: String::new(),
            warnings: vec![
                format!(
                    "Đĩa {} ({}) sẽ được khởi tạo với bảng phân vùng GPT.",
                    disk.number,
                    human_bytes(disk.size)
                ),
                "Mọi dữ liệu còn lại trên đĩa sẽ không truy cập được.".into(),
            ],
        });
    }

    fn open_wipe(&mut self, disk: &DiskDto) {
        let mut warnings = vec![
            format!(
                "XOÁ SẠCH đĩa {} — {} ({})",
                disk.number,
                disk.name,
                human_bytes(disk.size)
            ),
            format!(
                "Toàn bộ {} phân vùng và dữ liệu sẽ bị xoá vĩnh viễn.",
                disk.partitions.len()
            ),
            "Không thể hoàn tác!".to_string(),
        ];
        let locked = disk.partitions.iter().filter(|p| p.bitlocker_on()).count();
        if locked > 0 {
            warnings.push(format!(
                "Có {locked} phân vùng đang bật BitLocker — nếu đang khoá, thao tác có thể thất bại."
            ));
        }
        self.dialog = Dialog::Confirm(ConfirmForm {
            action: Action::Wipe { disk: disk.number },
            keyword: "ERASE",
            input: String::new(),
            warnings,
        });
    }

    // -- Giao diện ----------------------------------------------------------

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("topbar")
            .frame(
                Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(Margin::symmetric(14, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("◆").color(theme::ACCENT).size(23.0).strong());
                    ui.label(
                        RichText::new("PerfectDS")
                            .color(theme::TEXT)
                            .size(20.0)
                            .strong(),
                    );
                    ui.label(
                        RichText::new("Trình quản lý & chia ổ đĩa")
                            .color(theme::MUTED)
                            .size(12.0),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.admin {
                            chip(ui, "QUẢN TRỊ VIÊN", theme::GREEN);
                        } else {
                            let b = ui.add(
                                egui::Button::new(
                                    RichText::new("Chạy với quyền Admin")
                                        .strong()
                                        .color(Color32::from_rgb(30, 24, 5)),
                                )
                                .fill(theme::AMBER)
                                .min_size(Vec2::new(0.0, 30.0)),
                            );
                            if b.clicked() {
                                self.elevate(ctx);
                            }
                        }
                        let refresh = ui.add_enabled(
                            self.busy.is_none(),
                            egui::Button::new("Làm mới").min_size(Vec2::new(0.0, 30.0)),
                        );
                        if refresh.clicked() {
                            self.start_load(ctx);
                        }
                        if ui.selectable_label(self.show_logs, "Nhật ký").clicked() {
                            self.show_logs = !self.show_logs;
                        }
                        if ui.button("Giới thiệu").clicked() {
                            self.dialog = Dialog::About;
                        }
                    });
                });
            });
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(300.0)
            .min_width(265.0)
            .max_width(400.0)
            .frame(
                Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(Margin::symmetric(12, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Ổ ĐĨA VẬT LÝ")
                            .color(theme::MUTED)
                            .size(11.0)
                            .strong(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{}", self.disks.len()))
                                .color(theme::MUTED)
                                .size(11.0),
                        );
                    });
                });
                ui.add_space(6.0);

                if self.disks.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        if self.busy.is_some() {
                            ui.add(egui::Spinner::new().size(22.0));
                            ui.add_space(6.0);
                            ui.label(RichText::new("Đang quét ổ đĩa…").color(theme::MUTED));
                        } else {
                            ui.label(
                                RichText::new("Không tìm thấy ổ đĩa nào.").color(theme::MUTED),
                            );
                        }
                    });
                    return;
                }

                let disks = self.disks.clone();
                let mut clicked = None;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for d in &disks {
                            if disk_card(ui, d, self.selected_disk == Some(d.number)) {
                                clicked = Some(d.number);
                            }
                            ui.add_space(6.0);
                        }
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Mọi thao tác ghi đều cần quyền Admin.\nNhấn F5 để làm mới.",
                            )
                            .color(theme::MUTED)
                            .size(11.0),
                        );
                    });
                if let Some(n) = clicked
                    && self.selected_disk != Some(n)
                {
                    self.selected_disk = Some(n);
                    self.selected_part = None;
                }
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("statusbar")
            .frame(
                Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(Margin::symmetric(12, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some(busy) = &self.busy {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(
                            RichText::new(format!(
                                "{} ({}s)",
                                busy.label,
                                busy.started.elapsed().as_secs()
                            ))
                            .color(theme::ACCENT)
                            .size(12.0),
                        );
                    } else if let Some(st) = &self.status {
                        let c = if st.ok { theme::GREEN } else { theme::RED };
                        ui.label(RichText::new("●").color(c).size(11.0));
                        ui.label(RichText::new(&st.text).color(theme::MUTED).size(12.0));
                    } else {
                        ui.label(RichText::new("Sẵn sàng").color(theme::MUTED).size(12.0));
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button("Mở thư mục nhật ký")
                            .on_hover_text(sys::log_file().to_string_lossy())
                            .clicked()
                        {
                            let _ = sys::open_path(&sys::log_dir());
                        }
                        ui.label(
                            RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                .color(theme::MUTED)
                                .size(11.0),
                        );
                    });
                });
            });
    }

    fn logs_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("logspanel")
            .resizable(true)
            .default_height(160.0)
            .min_height(90.0)
            .max_height(360.0)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(9, 13, 19))
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("NHẬT KÝ THAO TÁC")
                            .color(theme::MUTED)
                            .size(11.0)
                            .strong(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Xoá").clicked() {
                            self.logs.clear();
                        }
                        if ui.small_button("Sao chép").clicked() {
                            let all: String = self
                                .logs
                                .iter()
                                .map(|l| format!("[{}] {}", l.time, l.text))
                                .collect::<Vec<_>>()
                                .join("\n");
                            ui.ctx().copy_text(all);
                        }
                        if ui.small_button("Mở file").clicked() {
                            let _ = sys::open_path(&sys::log_file());
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &self.logs {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(&l.time)
                                        .monospace()
                                        .color(theme::MUTED)
                                        .size(11.0),
                                );
                                let c = if l.ok { theme::GREEN } else { theme::RED };
                                ui.label(RichText::new(&l.text).color(c).size(12.0));
                            });
                        }
                    });
            });
    }

    fn central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(14)))
            .show(ctx, |ui| {
                let Some(disk) = self.selected_disk_dto().cloned() else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(120.0);
                        if self.first_load {
                            ui.add(egui::Spinner::new().size(28.0));
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("Đang đọc cấu hình ổ đĩa…")
                                    .color(theme::MUTED)
                                    .size(14.0),
                            );
                        } else {
                            ui.label(
                                RichText::new("Chọn một ổ đĩa ở cột bên trái")
                                    .color(theme::TEXT)
                                    .size(18.0)
                                    .strong(),
                            );
                            ui.label(
                                RichText::new("PerfectDS sẽ hiển thị bố cục và cho phép chia ổ.")
                                    .color(theme::MUTED)
                                    .size(13.0),
                            );
                        }
                    });
                    return;
                };

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if !self.admin {
                            if admin_banner(ui) {
                                self.elevate(ctx);
                            }
                            ui.add_space(8.0);
                        }
                        self.disk_header(ui, &disk);
                        ui.add_space(10.0);
                        stats_row(ui, &disk);
                        ui.add_space(10.0);
                        if let Some(n) = disk_map_card(ui, &disk, self.selected_part) {
                            self.selected_part = Some(n);
                        }
                        ui.add_space(10.0);
                        self.toolbar(ui, &disk);
                        ui.add_space(10.0);
                        self.partitions_card(ui, &disk);
                        ui.add_space(10.0);
                        self.free_space_card(ui, &disk);
                        ui.add_space(16.0);
                    });
            });
    }

    fn disk_header(&self, ui: &mut egui::Ui, disk: &DiskDto) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&disk.name).size(23.0).strong());
                    chip(ui, &format!("ĐĨA {}", disk.number), theme::ACCENT);
                    chip(ui, &disk.style, theme::BLUE);
                    if disk.protected() {
                        chip(ui, "HỆ THỐNG", theme::AMBER);
                    }
                    if !disk.healthy() {
                        chip(ui, &disk.health, theme::RED);
                    }
                });
                ui.add_space(2.0);
                ui.label(
                    RichText::new(format!(
                        "Serial: {}   ·   {}   ·   {}",
                        if disk.serial.is_empty() {
                            "—"
                        } else {
                            &disk.serial
                        },
                        if disk.bus_type.is_empty() {
                            "Không rõ"
                        } else {
                            &disk.bus_type
                        },
                        if disk.operational.is_empty() {
                            "Online"
                        } else {
                            &disk.operational
                        }
                    ))
                    .color(theme::MUTED)
                    .size(11.5),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("DUNG LƯỢNG TRỐNG")
                            .color(theme::MUTED)
                            .size(10.5)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(human_bytes(disk.free_bytes()))
                            .color(theme::GREEN)
                            .size(18.0)
                            .strong(),
                    );
                });
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, disk: &DiskDto) {
        let active = self.admin && self.busy.is_none();
        let gaps = disk.usable_gaps();
        let sel = self.selected_partition();
        let sel_ok = sel.as_ref().filter(|p| !p.is_protected());

        ui.horizontal_wrapped(|ui| {
            let can_create = active && disk.style != "RAW" && !gaps.is_empty();
            let b = ui.add_enabled(
                can_create,
                egui::Button::new(RichText::new("+  Tạo phân vùng").strong())
                    .fill(theme::ACCENT_DEEP)
                    .min_size(Vec2::new(0.0, 32.0)),
            );
            if b.on_disabled_hover_text(if disk.style == "RAW" {
                "Đĩa chưa khởi tạo — hãy khởi tạo đĩa trước."
            } else if gaps.is_empty() {
                "Không có vùng trống đủ lớn (≥ 16 MB)."
            } else if !self.admin {
                "Cần quyền Administrator."
            } else {
                ""
            })
            .clicked()
                && let Some(g) = gaps.iter().max_by_key(|g| g.size)
            {
                self.open_create(disk, *g);
            }

            let can_split = active
                && sel_ok.as_ref().map(|p| p.can_shrink()).unwrap_or(false)
                && sel_ok
                    .as_ref()
                    .map(|p| ops::max_split_size(p) >= 256 * MIB)
                    .unwrap_or(false);
            let b = ui.add_enabled(
                can_split,
                egui::Button::new("Chia ổ").min_size(Vec2::new(0.0, 32.0)),
            );
            if b.on_disabled_hover_text("Chọn một phân vùng NTFS/ReFS còn đủ dung lượng để chia.")
                .clicked()
                && let Some(p) = &sel
            {
                self.open_split(p);
            }

            let can_extend = active && sel_ok.as_ref().map(|p| p.can_extend()).unwrap_or(false);
            let b = ui.add_enabled(
                can_extend,
                egui::Button::new("Mở rộng").min_size(Vec2::new(0.0, 32.0)),
            );
            if b.on_disabled_hover_text("Chọn phân vùng có vùng trống liền kề (cần quyền Admin).")
                .clicked()
                && let Some(p) = &sel
            {
                self.open_extend(p);
            }

            ui.separator();

            let can_format = active
                && sel_ok
                    .as_ref()
                    .map(|p| p.can_format() && !p.bitlocker_on())
                    .unwrap_or(false);
            let b = ui.add_enabled(
                can_format,
                egui::Button::new("Định dạng").min_size(Vec2::new(0.0, 32.0)),
            );
            if b.on_disabled_hover_text("Chọn phân vùng không phải hệ thống và đã tắt BitLocker.")
                .clicked()
                && let Some(p) = &sel
            {
                self.open_format(p);
            }

            let can_letter = active && sel_ok.is_some();
            let b = ui.add_enabled(
                can_letter,
                egui::Button::new("Đổi ký tự ổ").min_size(Vec2::new(0.0, 32.0)),
            );
            if b.on_disabled_hover_text("Chọn một phân vùng để gán ký tự ổ.")
                .clicked()
                && let Some(p) = &sel
            {
                self.open_letter(p);
            }

            ui.separator();

            let can_delete = active && sel_ok.is_some();
            let b = ui.add_enabled(
                can_delete,
                egui::Button::new(RichText::new("Xoá phân vùng").color(theme::RED))
                    .fill(theme::RED_DEEP)
                    .min_size(Vec2::new(0.0, 32.0)),
            );
            if b.on_disabled_hover_text("Không thể xoá phân vùng hệ thống/dự phòng.")
                .clicked()
                && let Some(p) = &sel
            {
                self.open_delete(p);
            }

            let can_init = active && disk.style == "RAW";
            let b = ui.add_enabled(
                can_init,
                egui::Button::new("Khởi tạo đĩa").min_size(Vec2::new(0.0, 32.0)),
            );
            if b.on_disabled_hover_text("Chỉ dùng cho đĩa RAW (chưa có bảng phân vùng).")
                .clicked()
            {
                self.open_initialize(disk);
            }

            let can_wipe = active && !disk.protected();
            let b = ui.add_enabled(
                can_wipe,
                egui::Button::new(RichText::new("Xoá sạch đĩa").color(theme::RED))
                    .stroke(Stroke::new(1.0_f32, theme::RED))
                    .min_size(Vec2::new(0.0, 32.0)),
            );
            if b.on_disabled_hover_text("Không thể xoá đĩa chứa hệ điều hành.")
                .clicked()
            {
                self.open_wipe(disk);
            }
        });

        ui.add_space(4.0);
        if let Some(p) = &sel {
            ui.label(
                RichText::new(format!(
                    "Đang chọn: #{} · {} · {} · {}",
                    p.number,
                    p.display_name(),
                    p.flags_text(),
                    human_bytes(p.size)
                ))
                .color(theme::MUTED)
                .size(11.0),
            );
        } else {
            ui.label(
                RichText::new("Mẹo: nhấn vào số phân vùng trong bảng để chọn thao tác.")
                    .color(theme::MUTED)
                    .size(11.0),
            );
        }
    }

    fn partitions_card(&mut self, ui: &mut egui::Ui, disk: &DiskDto) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("PHÂN VÙNG")
                        .color(theme::MUTED)
                        .size(11.0)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} phân vùng", disk.partitions.len()))
                            .color(theme::MUTED)
                            .size(11.0),
                    );
                });
            });
            ui.add_space(6.0);

            if disk.partitions.is_empty() {
                ui.label(
                    RichText::new("Đĩa chưa có phân vùng nào. Hãy tạo phân vùng mới.")
                        .color(theme::MUTED)
                        .size(12.0),
                );
                return;
            }

            let mut select = None;
            egui::Grid::new("partition_table")
                .num_columns(7)
                .striped(true)
                .spacing(Vec2::new(14.0, 8.0))
                .min_col_width(26.0)
                .show(ui, |ui| {
                    head(ui, "#");
                    head(ui, "Tên / Nhãn");
                    head(ui, "Hệ tệp");
                    head(ui, "Dung lượng");
                    head(ui, "Đã dùng");
                    head(ui, "Ký tự");
                    head(ui, "Loại / Cờ");
                    ui.end_row();

                    for p in &disk.partitions {
                        let selected = self.selected_part == Some(p.number);
                        if ui
                            .add(egui::SelectableLabel::new(
                                selected,
                                RichText::new(format!("#{}", p.number)).strong(),
                            ))
                            .clicked()
                        {
                            select = Some(p.number);
                        }
                        if ui
                            .add(egui::SelectableLabel::new(selected, p.display_name()))
                            .clicked()
                        {
                            select = Some(p.number);
                        }
                        ui.label(
                            RichText::new(if p.fs.is_empty() {
                                "—".to_string()
                            } else {
                                p.fs.clone()
                            })
                            .color(if p.fs.is_empty() {
                                theme::MUTED
                            } else {
                                theme::TEXT
                            })
                            .size(12.0),
                        );
                        ui.label(RichText::new(human_bytes(p.size)).size(12.0));
                        ui.add(
                            egui::ProgressBar::new(p.usage_ratio())
                                .desired_width(130.0)
                                .text(
                                    RichText::new(format!(
                                        "{} / {}",
                                        human_bytes(p.used()),
                                        human_bytes(p.size)
                                    ))
                                    .size(10.0),
                                ),
                        );
                        ui.label(
                            RichText::new(
                                p.letter_opt()
                                    .map(|c| format!("{c}:"))
                                    .unwrap_or_else(|| "—".into()),
                            )
                            .color(if p.letter_opt().is_some() {
                                theme::ACCENT
                            } else {
                                theme::MUTED
                            })
                            .size(12.0),
                        );
                        let mut tail = p.kind.clone();
                        let flags = p.flags_text();
                        if !flags.is_empty() {
                            tail = format!("{tail} · {flags}");
                        }
                        ui.label(RichText::new(tail).color(theme::MUTED).size(11.0));
                        ui.end_row();
                    }
                });
            if let Some(n) = select {
                self.selected_part = Some(n);
            }

            if let Some(p) = self.selected_partition() {
                ui.add_space(10.0);
                Frame::new()
                    .fill(theme::CARD_HOVER)
                    .stroke(Stroke::new(1.0_f32, theme::ACCENT_DEEP))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!("#{} · {}", p.number, p.display_name()))
                                    .strong()
                                    .color(theme::TEXT)
                                    .size(13.0),
                            );
                            if let Some(c) = p.letter_opt() {
                                chip(ui, &format!("{c}:"), theme::ACCENT);
                            }
                            chip(
                                ui,
                                if p.fs.is_empty() {
                                    "Chưa định dạng"
                                } else {
                                    &p.fs
                                },
                                theme::BLUE,
                            );
                            chip(ui, &p.kind, theme::PURPLE);
                            if p.bitlocker_on() {
                                chip(ui, "BITLOCKER", theme::AMBER);
                            }
                            if p.is_boot || p.is_system {
                                chip(ui, "BẢO VỆ", theme::AMBER);
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "Bắt đầu {}   ·   Kết thúc {}   ·   Trống {}",
                                human_bytes(p.offset),
                                human_bytes(p.offset + p.size),
                                human_bytes(p.free)
                            ))
                            .color(theme::MUTED)
                            .size(11.0),
                        );
                        if p.min_size > 0 || p.max_size > 0 {
                            ui.label(
                                RichText::new(format!(
                                    "Windows cho phép: tối thiểu {} · tối đa {}",
                                    if p.min_size > 0 {
                                        human_bytes(p.min_size)
                                    } else {
                                        "—".into()
                                    },
                                    if p.max_size > 0 {
                                        human_bytes(p.max_size)
                                    } else {
                                        "—".into()
                                    }
                                ))
                                .color(theme::MUTED)
                                .size(11.0),
                            );
                        }
                    });
            }
        });
    }

    fn free_space_card(&mut self, ui: &mut egui::Ui, disk: &DiskDto) {
        card(ui, |ui| {
            let gaps = disk.usable_gaps();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("VÙNG TRỐNG KHẢ DỤNG")
                        .color(theme::MUTED)
                        .size(11.0)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "Tổng {}",
                            human_bytes(gaps.iter().map(|g| g.size).sum::<u64>())
                        ))
                        .color(theme::GREEN)
                        .size(11.0)
                        .strong(),
                    );
                });
            });
            ui.add_space(4.0);
            if gaps.is_empty() {
                ui.label(
                    RichText::new("Không có vùng trống nào đủ lớn (≥ 16 MB).")
                        .color(theme::MUTED)
                        .size(12.0),
                );
                return;
            }
            for (i, g) in gaps.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new("▱").color(theme::GREEN).size(16.0));
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} trống · bắt đầu {}",
                                human_bytes(g.size),
                                human_bytes(g.start)
                            ))
                            .color(theme::TEXT)
                            .size(12.0),
                        );
                        ui.label(
                            RichText::new("Vị trí đã căn chỉnh 1 MB — dùng được ngay")
                                .color(theme::MUTED)
                                .size(10.5),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let enabled = self.admin && self.busy.is_none();
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new("+ Tạo ở đây")
                                    .fill(theme::ACCENT_DEEP)
                                    .min_size(Vec2::new(0.0, 26.0)),
                            )
                            .on_disabled_hover_text("Cần quyền Administrator.")
                            .clicked()
                        {
                            self.open_create(disk, *g);
                        }
                    });
                });
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if matches!(self.dialog, Dialog::None) {
            return;
        }
        let mut dlg = std::mem::replace(&mut self.dialog, Dialog::None);
        let outcome = match &mut dlg {
            Dialog::Create(f) => show_create(ctx, f, &self.disks),
            Dialog::Split(f) => show_split(ctx, f),
            Dialog::Extend(f) => show_extend(ctx, f),
            Dialog::Format(f) => show_format(ctx, f),
            Dialog::Confirm(f) => show_confirm(ctx, f),
            Dialog::Letter(f) => show_letter(ctx, f),
            Dialog::About => show_about(ctx),
            Dialog::Result(r) => show_result(ctx, r),
            Dialog::None => Outcome::None,
        };
        match outcome {
            Outcome::None => self.dialog = dlg,
            Outcome::Close => {}
            Outcome::Start(action) => self.start_action(ctx, action),
            Outcome::OpenLog => {
                let _ = sys::open_path(&sys::log_file());
                self.dialog = dlg;
            }
            Outcome::Copy(text) => {
                ctx.copy_text(text);
                self.dialog = dlg;
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);

        let status_expired = self
            .status
            .as_ref()
            .is_some_and(|st| st.at.elapsed() > Duration::from_secs(10));
        if status_expired {
            self.status = None;
        }
        if self.busy.is_none() && ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.start_load(ctx);
        }

        self.top_bar(ctx);
        self.sidebar(ctx);
        self.status_bar(ctx);
        if self.show_logs {
            self.logs_panel(ctx);
        }
        self.central_panel(ctx);
        self.dialogs(ctx);

        if self.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }
}

fn p_disk(app: &App, _p: &model::PartitionDto) -> u32 {
    app.selected_disk.unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Widgets phụ
// ---------------------------------------------------------------------------

fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    let bg = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 32);
    Frame::new()
        .fill(bg)
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 110),
        ))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).size(10.0).strong());
        });
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0_f32, theme::BORDER))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn head(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::MUTED).size(10.5).strong());
}

fn disk_card(ui: &mut egui::Ui, d: &DiskDto, selected: bool) -> bool {
    let fill = if selected {
        theme::CARD_HOVER
    } else {
        theme::CARD
    };
    let stroke = if selected {
        Stroke::new(1.6_f32, theme::ACCENT)
    } else {
        Stroke::new(1.0_f32, theme::BORDER)
    };
    let inner = Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}", d.number))
                        .monospace()
                        .strong()
                        .color(theme::ACCENT)
                        .size(14.0),
                );
                ui.add(egui::Label::new(RichText::new(&d.name).strong().size(13.0)).truncate());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if d.protected() {
                        chip(ui, "SYS", theme::AMBER);
                    }
                    if !d.healthy() {
                        chip(ui, "LỖI", theme::RED);
                    }
                });
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} · {} · {}",
                        human_bytes(d.size),
                        if d.style.is_empty() { "?" } else { &d.style },
                        if d.bus_type.is_empty() {
                            "—"
                        } else {
                            &d.bus_type
                        }
                    ))
                    .color(theme::MUTED)
                    .size(10.5),
                );
            });
            ui.horizontal(|ui| {
                let free = d.free_bytes();
                if free > 0 {
                    ui.label(
                        RichText::new(format!("{} trống", human_bytes(free)))
                            .color(theme::GREEN)
                            .size(10.5),
                    );
                } else {
                    ui.label(
                        RichText::new("Không còn vùng trống")
                            .color(theme::MUTED)
                            .size(10.5),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} phân vùng", d.partitions.len()))
                            .color(theme::MUTED)
                            .size(10.5),
                    );
                });
            });
        })
        .response;
    let resp = inner.interact(Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

fn admin_banner(ui: &mut egui::Ui) -> bool {
    let mut clicked = false;
    Frame::new()
        .fill(Color32::from_rgb(48, 38, 8))
        .stroke(Stroke::new(1.0_f32, theme::AMBER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("!").color(theme::AMBER).size(17.0).strong());
                ui.label(
                    RichText::new(
                        "Chế độ chỉ đọc: cần quyền Administrator để tạo, chia, xoá hoặc định dạng ổ đĩa.",
                    )
                    .color(theme::TEXT)
                    .size(12.5),
                );
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("Nâng quyền ngay")
                                .strong()
                                .color(Color32::from_rgb(30, 24, 5)),
                        )
                        .fill(theme::AMBER),
                    )
                    .clicked()
                {
                    clicked = true;
                }
            });
        });
    clicked
}

fn stat_card(ui: &mut egui::Ui, title: &str, value: &str, color: Color32, sub: &str) {
    Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0_f32, theme::BORDER))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical(|ui| {
                ui.label(RichText::new(title).color(theme::MUTED).size(10.0).strong());
                ui.add_space(2.0);
                ui.label(RichText::new(value).color(color).size(17.0).strong());
                if !sub.is_empty() {
                    ui.label(RichText::new(sub).color(theme::MUTED).size(10.0));
                }
            });
        });
}

fn stats_row(ui: &mut egui::Ui, disk: &DiskDto) {
    let width = ui.available_width();
    let gap = 8.0;
    let w = ((width - gap * 3.0) / 4.0).max(100.0);
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(Vec2::new(w, 66.0), Layout::top_down(Align::Min), |ui| {
            stat_card(
                ui,
                "DUNG LƯỢNG ĐĨA",
                &human_bytes(disk.size),
                theme::TEXT,
                &format!("{} phân vùng", disk.partitions.len()),
            );
        });
        ui.allocate_ui_with_layout(Vec2::new(w, 66.0), Layout::top_down(Align::Min), |ui| {
            stat_card(
                ui,
                "BẢNG PHÂN VÙNG",
                &disk.style,
                theme::BLUE,
                "GPT khuyến nghị cho ổ > 2 TB",
            );
        });
        ui.allocate_ui_with_layout(Vec2::new(w, 66.0), Layout::top_down(Align::Min), |ui| {
            stat_card(
                ui,
                "VÙNG TRỐNG KHẢ DỤNG",
                &human_bytes(disk.free_bytes()),
                theme::GREEN,
                &format!("{} vùng có thể dùng", disk.usable_gaps().len()),
            );
        });
        ui.allocate_ui_with_layout(Vec2::new(w, 66.0), Layout::top_down(Align::Min), |ui| {
            stat_card(
                ui,
                "TÌNH TRẠNG",
                if disk.healthy() {
                    "Tốt"
                } else {
                    &disk.health
                },
                if disk.healthy() {
                    theme::GREEN
                } else {
                    theme::RED
                },
                if disk.is_read_only {
                    "Chỉ đọc"
                } else if disk.is_offline {
                    "Offline"
                } else {
                    "Sẵn sàng"
                },
            );
        });
    });
}

#[derive(Clone)]
struct MapSeg {
    start: u64,
    end: u64,
    number: Option<u32>,
}

fn disk_map_card(ui: &mut egui::Ui, disk: &DiskDto, _selected: Option<u32>) -> Option<u32> {
    let mut clicked = None;
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("BỐ CỤC ĐĨA")
                    .color(theme::MUTED)
                    .size(11.0)
                    .strong(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                legend_item(ui, theme::GREEN, "Trống");
                legend_item(ui, theme::PURPLE, "Dự phòng");
                legend_item(ui, theme::AMBER, "Hệ thống");
                legend_item(ui, theme::BLUE, "Dữ liệu");
            });
        });
        ui.add_space(8.0);
        clicked = draw_disk_map(ui, disk, _selected);
    });
    clicked
}

fn legend_item(ui: &mut egui::Ui, color: Color32, text: &str) {
    ui.label(RichText::new("■").color(color).size(10.0));
    ui.label(RichText::new(text).color(theme::MUTED).size(10.5));
    ui.add_space(4.0);
}

fn draw_disk_map(ui: &mut egui::Ui, disk: &DiskDto, selected: Option<u32>) -> Option<u32> {
    let width = ui.available_width().max(100.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 82.0), Sense::click());
    let painter = ui.painter();

    painter.rect_filled(rect, CornerRadius::same(7), Color32::from_rgb(8, 11, 16));

    let bar = Rect::from_min_max(
        egui::pos2(rect.left() + 8.0, rect.top() + 10.0),
        egui::pos2(rect.right() - 8.0, rect.bottom() - 26.0),
    );
    if disk.size == 0 {
        return None;
    }

    // Ghép các đoạn phân vùng và vùng trống theo thứ tự.
    let mut segs: Vec<MapSeg> = Vec::new();
    for p in &disk.partitions {
        segs.push(MapSeg {
            start: p.offset,
            end: p.offset.saturating_add(p.size),
            number: Some(p.number),
        });
    }
    for g in disk.all_gaps() {
        segs.push(MapSeg {
            start: g.start,
            end: g.end(),
            number: None,
        });
    }
    segs.sort_by_key(|s| s.start);

    let x_of = |off: u64| -> f32 {
        bar.left() + (off.min(disk.size) as f64 / disk.size as f64) as f32 * bar.width()
    };

    let hover_pos = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| rect.contains(*p));
    let mut hovered: Option<(String, String, Color32)> = None;

    for seg in &segs {
        if seg.end <= seg.start {
            continue;
        }
        let x0 = x_of(seg.start);
        let x1 = x_of(seg.end);
        if x1 - x0 < 0.6 {
            continue;
        }
        let r = Rect::from_min_max(
            egui::pos2(x0 + 0.8, bar.top()),
            egui::pos2((x1 - 0.8).max(x0 + 1.0), bar.bottom()),
        );
        let color = match seg.number {
            None => Color32::from_rgb(26, 46, 38),
            Some(n) => partition_color(disk.partition(n)),
        };
        painter.rect_filled(r, CornerRadius::same(3), color);
        if let Some(n) = seg.number {
            if selected == Some(n) {
                painter.rect_stroke(
                    r,
                    CornerRadius::same(3),
                    Stroke::new(2.0_f32, Color32::WHITE),
                    StrokeKind::Inside,
                );
            }
            if r.width() > 40.0 {
                let p = disk.partition(n);
                let label = p
                    .and_then(|p| p.letter_opt())
                    .map(|c| format!("{c}:"))
                    .unwrap_or_else(|| format!("#{n}"));
                painter.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(10, 14, 20),
                );
            }
        }
        if let Some(pos) = hover_pos
            && pos.x >= r.left()
            && pos.x <= r.right()
            && pos.y >= bar.top()
            && pos.y <= bar.bottom()
        {
            if let Some(n) = seg.number {
                if let Some(p) = disk.partition(n) {
                    hovered = Some((
                        format!("#{} · {}", p.number, p.display_name()),
                        format!(
                            "{} · {} · {} · {}",
                            human_bytes(p.size),
                            if p.fs.is_empty() {
                                "chưa định dạng"
                            } else {
                                &p.fs
                            },
                            p.letter_opt()
                                .map(|c| format!("ổ {c}:"))
                                .unwrap_or_else(|| "không có ký tự".into()),
                            if p.free > 0 {
                                format!("{} trống", human_bytes(p.free))
                            } else {
                                p.flags_text()
                            }
                        ),
                        partition_color(Some(p)),
                    ));
                }
            } else if let Some(g) = disk
                .usable_gaps()
                .iter()
                .find(|g| seg.start >= g.start && seg.start < g.end())
            {
                hovered = Some((
                    "Vùng trống".into(),
                    format!("{} có thể tạo phân vùng mới", human_bytes(g.size)),
                    theme::GREEN,
                ));
            } else {
                hovered = Some((
                    "Vùng chưa phân bổ".into(),
                    format!(
                        "{} (không đủ điều kiện tạo mới)",
                        human_bytes(seg.end - seg.start)
                    ),
                    theme::MUTED,
                ));
            }
        }
    }

    // Thước đo hai đầu.
    painter.text(
        egui::pos2(bar.left(), rect.bottom() - 9.0),
        Align2::LEFT_CENTER,
        "0",
        egui::FontId::proportional(10.0),
        theme::MUTED,
    );
    painter.text(
        egui::pos2(bar.center().x, rect.bottom() - 9.0),
        Align2::CENTER_CENTER,
        format!("{} / {}", human_bytes(disk.size), disk.style),
        egui::FontId::proportional(10.0),
        theme::MUTED,
    );
    painter.text(
        egui::pos2(bar.right(), rect.bottom() - 9.0),
        Align2::RIGHT_CENTER,
        human_bytes(disk.size),
        egui::FontId::proportional(10.0),
        theme::MUTED,
    );

    if let Some((title, body, color)) = hovered.clone() {
        let resp = resp.clone();
        resp.on_hover_ui(move |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("■").color(color).size(10.0));
                ui.label(RichText::new(title).strong().size(12.0));
            });
            ui.label(RichText::new(body).color(theme::MUTED).size(11.0));
        });
    }

    if resp.clicked()
        && let Some(pos) = resp
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.hover_pos()))
    {
        for seg in &segs {
            if let Some(n) = seg.number {
                let x0 = x_of(seg.start);
                let x1 = x_of(seg.end);
                if pos.x >= x0 && pos.x <= x1 && pos.y >= bar.top() && pos.y <= bar.bottom() {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn partition_color(p: Option<&model::PartitionDto>) -> Color32 {
    let Some(p) = p else {
        return theme::MUTED;
    };
    if p.is_system || p.is_boot {
        theme::AMBER
    } else if p.kind.eq_ignore_ascii_case("reserved") {
        theme::PURPLE
    } else if p.fs.is_empty() {
        theme::MUTED
    } else {
        theme::BLUE
    }
}

// ---------------------------------------------------------------------------
// Hộp thoại
// ---------------------------------------------------------------------------

fn dialog_frame(
    ctx: &egui::Context,
    title: &str,
    width: f32,
    add: impl FnOnce(&mut egui::Ui) -> Outcome,
) -> Outcome {
    // Lớp phủ tối + chặn tương tác phía sau (hộp thoại ở lớp Foreground).
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("perfectds_dim"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
            ui.painter()
                .rect_filled(rect, CornerRadius::same(0), Color32::from_black_alpha(175));
        });
    let mut out = Outcome::None;
    egui::Window::new(title)
        .id(egui::Id::new(("perfectds_dialog", title)))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .default_width(width)
        .frame(
            Frame::window(&ctx.style())
                .fill(theme::PANEL)
                .stroke(Stroke::new(1.0_f32, theme::BORDER))
                .corner_radius(CornerRadius::same(12)),
        )
        .show(ctx, |ui| {
            ui.set_width(width - 24.0);
            out = add(ui);
        });
    out
}

fn show_create(ctx: &egui::Context, f: &mut CreateForm, disks: &[DiskDto]) -> Outcome {
    dialog_frame(ctx, "Tạo phân vùng mới", 470.0, |ui| {
        let mut out = Outcome::None;
        info_line(ui, "Đĩa", &format!("{}", f.disk));
        info_line(ui, "Vị trí bắt đầu", &human_bytes(f.offset));
        info_line(ui, "Tối đa", &human_bytes(f.max));
        ui.add_space(8.0);

        let max_mib = (f.max / MIB).max(16);
        egui::Grid::new("create_form")
            .num_columns(2)
            .spacing(Vec2::new(14.0, 8.0))
            .show(ui, |ui| {
                ui.label("Dung lượng");
                ui.horizontal(|ui| {
                    let mut v = f.size_mib;
                    ui.add(
                        egui::DragValue::new(&mut v)
                            .range(16..=max_mib)
                            .speed(128.0)
                            .suffix(" MB"),
                    );
                    f.size_mib = v.clamp(16, max_mib);
                    ui.label(
                        RichText::new(format!("≈ {}", human_bytes(f.size_mib * MIB)))
                            .color(theme::MUTED)
                            .size(11.0),
                    );
                });
                ui.end_row();

                ui.label("Hệ tệp");
                fs_combo(ui, "create_fs", &mut f.fs);
                ui.end_row();

                ui.label("Ký tự ổ");
                letter_combo(ui, "create_letter", &mut f.letter, disks);
                ui.end_row();

                ui.label("Nhãn ổ đĩa");
                ui.add(
                    egui::TextEdit::singleline(&mut f.label)
                        .hint_text("DATA")
                        .desired_width(240.0),
                );
                ui.end_row();
            });

        ui.add_space(10.0);
        warning_box(
            ui,
            "Phân vùng mới sẽ được định dạng và mọi dữ liệu trong vùng trống này (nếu có) sẽ bị xoá.",
            theme::AMBER,
        );
        ui.add_space(10.0);
        buttons(
            ui,
            "Huỷ",
            "Tạo phân vùng",
            theme::ACCENT_DEEP,
            true,
            &mut out,
            || {
                Outcome::Start(Action::Create {
                    disk: f.disk,
                    offset: f.offset,
                    size: f.size_mib * MIB,
                    fs: f.fs,
                    label: f.label.trim().to_string(),
                    letter: f.letter,
                })
            },
        );
        out
    })
}

fn show_split(ctx: &egui::Context, f: &mut SplitForm) -> Outcome {
    dialog_frame(ctx, "Chia ổ đĩa", 500.0, |ui| {
        let mut out = Outcome::None;
        let p = &f.partition;
        info_line(
            ui,
            "Phân vùng nguồn",
            &format!(
                "#{} · {} · {}",
                p.number,
                p.display_name(),
                human_bytes(p.size)
            ),
        );
        info_line(ui, "Có thể tách tối đa", &human_bytes(f.max));
        ui.add_space(8.0);

        let max_mib = (f.max / MIB).max(256);
        egui::Grid::new("split_form")
            .num_columns(2)
            .spacing(Vec2::new(14.0, 8.0))
            .show(ui, |ui| {
                ui.label("Dung lượng ổ mới");
                ui.horizontal(|ui| {
                    let mut v = f.size_mib;
                    ui.add(
                        egui::DragValue::new(&mut v)
                            .range(256..=max_mib)
                            .speed(128.0)
                            .suffix(" MB"),
                    );
                    f.size_mib = v.clamp(256, max_mib);
                    ui.label(
                        RichText::new(format!("≈ {}", human_bytes(f.size_mib * MIB)))
                            .color(theme::MUTED)
                            .size(11.0),
                    );
                });
                ui.end_row();

                ui.label("Hệ tệp");
                fs_combo(ui, "split_fs", &mut f.fs);
                ui.end_row();

                ui.label("Ký tự ổ mới");
                letter_combo(ui, "split_letter", &mut f.letter, &[]);
                ui.end_row();

                ui.label("Nhãn ổ mới");
                ui.add(
                    egui::TextEdit::singleline(&mut f.label)
                        .hint_text("DATA2")
                        .desired_width(240.0),
                );
                ui.end_row();
            });

        ui.add_space(8.0);
        let keep = p.size.saturating_sub(f.size_mib * MIB);
        warning_box(
            ui,
            &format!(
                "Phân vùng #{} sẽ được thu nhỏ còn {} để nhường chỗ cho ổ mới. Nên sao lưu dữ liệu quan trọng trước khi chia.",
                p.number,
                human_bytes(keep)
            ),
            theme::AMBER,
        );
        ui.add_space(10.0);
        buttons(
            ui,
            "Huỷ",
            "Chia ổ",
            theme::ACCENT_DEEP,
            keep >= 256 * MIB,
            &mut out,
            || {
                Outcome::Start(Action::Split {
                    disk: f.disk,
                    partition: f.partition.number,
                    new_size: f.size_mib * MIB,
                    fs: f.fs,
                    label: f.label.trim().to_string(),
                    letter: f.letter,
                })
            },
        );
        out
    })
}

fn show_extend(ctx: &egui::Context, f: &mut ExtendForm) -> Outcome {
    dialog_frame(ctx, "Mở rộng phân vùng", 470.0, |ui| {
        let mut out = Outcome::None;
        let p = &f.partition;
        info_line(
            ui,
            "Phân vùng",
            &format!(
                "#{} · {} · {}",
                p.number,
                p.display_name(),
                human_bytes(p.size)
            ),
        );
        info_line(ui, "Tối đa", &human_bytes(f.max));
        ui.add_space(8.0);

        let min_mib = (p.size / MIB).max(1);
        let max_mib = (f.max / MIB).max(min_mib);
        egui::Grid::new("extend_form")
            .num_columns(2)
            .spacing(Vec2::new(14.0, 8.0))
            .show(ui, |ui| {
                ui.label("Kích thước mới");
                ui.horizontal(|ui| {
                    let mut v = f.size_mib;
                    ui.add(
                        egui::DragValue::new(&mut v)
                            .range(min_mib..=max_mib)
                            .speed(128.0)
                            .suffix(" MB"),
                    );
                    f.size_mib = v.clamp(min_mib, max_mib);
                    ui.label(
                        RichText::new(format!("≈ {}", human_bytes(f.size_mib * MIB)))
                            .color(theme::MUTED)
                            .size(11.0),
                    );
                });
                ui.end_row();
            });

        ui.add_space(8.0);
        warning_box(
            ui,
            "Windows chỉ mở rộng được khi có vùng trống liền ngay sau phân vùng. Phân vùng NTFS sẽ được mở rộng trực tiếp, không mất dữ liệu.",
            theme::GREEN,
        );
        ui.add_space(10.0);
        buttons(
            ui,
            "Huỷ",
            "Mở rộng",
            theme::ACCENT_DEEP,
            f.size_mib * MIB > p.size,
            &mut out,
            || {
                Outcome::Start(Action::Extend {
                    disk: f.disk,
                    partition: f.partition.number,
                    size: f.size_mib * MIB,
                })
            },
        );
        out
    })
}

fn show_format(ctx: &egui::Context, f: &mut FormatForm) -> Outcome {
    dialog_frame(ctx, "Định dạng phân vùng", 470.0, |ui| {
        let mut out = Outcome::None;
        let p = &f.partition;
        info_line(
            ui,
            "Phân vùng",
            &format!(
                "#{} · {} · {} · {}",
                p.number,
                p.display_name(),
                human_bytes(p.size),
                if p.fs.is_empty() {
                    "chưa định dạng"
                } else {
                    &p.fs
                }
            ),
        );
        ui.add_space(8.0);

        egui::Grid::new("format_form")
            .num_columns(2)
            .spacing(Vec2::new(14.0, 8.0))
            .show(ui, |ui| {
                ui.label("Hệ tệp");
                fs_combo(ui, "format_fs", &mut f.fs);
                ui.end_row();
                ui.label("Nhãn ổ đĩa");
                ui.add(
                    egui::TextEdit::singleline(&mut f.label)
                        .hint_text("DATA")
                        .desired_width(240.0),
                );
                ui.end_row();
            });
        ui.checkbox(&mut f.full, "Định dạng đầy đủ (chậm, kiểm tra bề mặt đĩa)");

        ui.add_space(8.0);
        if p.bitlocker_on() {
            warning_box(
                ui,
                "Phân vùng đang bật BitLocker. Hãy tắt BitLocker trước khi định dạng.",
                theme::AMBER,
            );
            ui.add_space(5.0);
        }
        warning_box(
            ui,
            "TOÀN BỘ dữ liệu trên phân vùng này sẽ bị xoá vĩnh viễn và không thể hoàn tác.",
            theme::RED,
        );
        ui.add_space(8.0);
        keyword_confirm(ui, "FORMAT", &mut f.input, theme::RED);
        ui.add_space(10.0);
        let ok = f.input.trim().eq_ignore_ascii_case("FORMAT");
        buttons(
            ui,
            "Huỷ",
            "Định dạng",
            theme::RED_DEEP,
            ok,
            &mut out,
            || {
                Outcome::Start(Action::Format {
                    disk: f.disk,
                    partition: f.partition.number,
                    fs: f.fs,
                    label: f.label.trim().to_string(),
                    full: f.full,
                })
            },
        );
        out
    })
}

fn show_confirm(ctx: &egui::Context, f: &mut ConfirmForm) -> Outcome {
    let title = f.action.title();
    dialog_frame(ctx, title, 480.0, |ui| {
        let mut out = Outcome::None;
        ui.label(
            RichText::new(f.action.description())
                .color(theme::TEXT)
                .size(13.0)
                .strong(),
        );
        ui.add_space(8.0);
        for w in &f.warnings {
            warning_box(ui, w, theme::RED);
            ui.add_space(5.0);
        }
        ui.add_space(4.0);
        keyword_confirm(ui, f.keyword, &mut f.input, theme::RED);
        ui.add_space(10.0);
        let ok = f.input.trim().eq_ignore_ascii_case(f.keyword);
        buttons(
            ui,
            "Huỷ",
            "Xác nhận",
            theme::RED_DEEP,
            ok,
            &mut out,
            || Outcome::Start(f.action.clone()),
        );
        out
    })
}

fn show_letter(ctx: &egui::Context, f: &mut LetterForm) -> Outcome {
    dialog_frame(ctx, "Đổi ký tự ổ đĩa", 420.0, |ui| {
        let mut out = Outcome::None;
        info_line(
            ui,
            "Phân vùng",
            &format!(
                "#{} · {} · {}",
                f.partition.number,
                f.partition.display_name(),
                human_bytes(f.partition.size)
            ),
        );
        ui.add_space(8.0);
        ui.label("Ký tự ổ đĩa mới");
        egui::ComboBox::from_id_salt("letter_pick")
            .selected_text(format!("{}:", f.letter))
            .width(140.0)
            .show_ui(ui, |ui| {
                for l in &f.letters {
                    ui.selectable_value(&mut f.letter, *l, format!("{l}:"));
                }
            });
        ui.add_space(8.0);
        warning_box(
            ui,
            "Đổi ký tự ổ có thể làm gián đoạn các chương trình đang dùng đường dẫn cũ. Dữ liệu không bị mất.",
            theme::AMBER,
        );
        ui.add_space(10.0);
        buttons(
            ui,
            "Huỷ",
            "Gán ký tự",
            theme::ACCENT_DEEP,
            !f.letters.is_empty(),
            &mut out,
            || {
                Outcome::Start(Action::SetLetter {
                    disk: f.disk,
                    partition: f.partition.number,
                    letter: f.letter,
                })
            },
        );
        out
    })
}

fn show_about(ctx: &egui::Context) -> Outcome {
    dialog_frame(ctx, "Giới thiệu PerfectDS", 520.0, |ui| {
        let mut out = Outcome::None;
        ui.label(
            RichText::new(format!("PerfectDS v{}", env!("CARGO_PKG_VERSION")))
                .size(17.0)
                .strong()
                .color(theme::ACCENT),
        );
        ui.label(
            RichText::new("Trình quản lý & chia ổ đĩa cho Windows")
                .color(theme::MUTED)
                .size(12.0),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new(
                "PerfectDS dùng Windows Storage API (cùng lớp mà Disk Management sử dụng) để:\n\
                 • Xem bố cục đĩa, phân vùng, vùng trống và dung lượng đã dùng.\n\
                 • Tạo / xoá phân vùng, chia ổ (thu nhỏ + tạo mới), mở rộng NTFS.\n\
                 • Định dạng NTFS / exFAT / FAT32, đổi ký tự ổ, khởi tạo GPT, xoá sạch đĩa.",
            )
            .color(theme::TEXT)
            .size(12.5),
        );
        ui.add_space(10.0);
        warning_box(
            ui,
            "Mọi thao tác ghi đều yêu cầu quyền Administrator, có hộp thoại xác nhận và được ghi vào nhật ký. Hãy sao lưu dữ liệu quan trọng trước khi chia ổ.",
            theme::AMBER,
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!("Nhật ký: {}", sys::log_file().display()))
                .color(theme::MUTED)
                .size(10.5),
        );
        ui.add_space(10.0);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 34.0),
            Layout::right_to_left(Align::Center),
            |ui| {
                if ui.button("Đóng").clicked() {
                    out = Outcome::Close;
                }
                if ui.button("Mở nhật ký").clicked() {
                    out = Outcome::OpenLog;
                }
            },
        );
        out
    })
}

fn show_result(ctx: &egui::Context, r: &mut ResultInfo) -> Outcome {
    let color = if r.ok { theme::GREEN } else { theme::RED };
    let title = if r.title.is_empty() {
        "Kết quả"
    } else {
        &r.title
    };
    dialog_frame(ctx, title, 560.0, |ui| {
        let mut out = Outcome::None;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if r.ok { "✔" } else { "✖" })
                    .color(color)
                    .size(22.0),
            );
            ui.label(
                RichText::new(if r.ok {
                    "Thao tác đã hoàn tất"
                } else {
                    "Đã xảy ra lỗi"
                })
                .color(color)
                .size(15.0)
                .strong(),
            );
        });
        ui.add_space(8.0);
        Frame::new()
            .fill(Color32::from_rgb(9, 13, 19))
            .stroke(Stroke::new(1.0_f32, theme::BORDER))
            .corner_radius(CornerRadius::same(7))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&r.body)
                                    .monospace()
                                    .size(11.5)
                                    .color(theme::TEXT),
                            )
                            .wrap(),
                        );
                    });
            });
        ui.add_space(10.0);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 34.0),
            Layout::right_to_left(Align::Center),
            |ui| {
                if ui.button("Đóng").clicked() {
                    out = Outcome::Close;
                }
                if ui.button("Sao chép").clicked() {
                    out = Outcome::Copy(r.body.clone());
                }
                if ui.button("Mở nhật ký").clicked() {
                    out = Outcome::OpenLog;
                }
            },
        );
        out
    })
}

// ---------------------------------------------------------------------------
// Thành phần nhỏ trong hộp thoại
// ---------------------------------------------------------------------------

fn info_line(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [130.0, 17.0],
            egui::Label::new(RichText::new(key).color(theme::MUTED).size(11.5)),
        );
        ui.label(RichText::new(value).color(theme::TEXT).size(11.5));
    });
}

fn fs_combo(ui: &mut egui::Ui, salt: &str, fs: &mut Fs) {
    egui::ComboBox::from_id_salt(salt)
        .selected_text(fs.as_str())
        .width(140.0)
        .show_ui(ui, |ui| {
            for f in Fs::all() {
                ui.selectable_value(fs, f, f.as_str());
            }
        });
}

fn letter_combo(ui: &mut egui::Ui, salt: &str, letter: &mut Option<char>, disks: &[DiskDto]) {
    let free = if disks.is_empty() {
        (b'D'..=b'Z').map(char::from).collect::<Vec<_>>()
    } else {
        model::free_letters(disks)
    };
    let text = letter
        .map(|c| format!("{c}:"))
        .unwrap_or_else(|| "Không gán".into());
    egui::ComboBox::from_id_salt(salt)
        .selected_text(text)
        .width(160.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(letter, None, "Không gán");
            for l in &free {
                ui.selectable_value(letter, Some(*l), format!("{l}:"));
            }
        });
}

fn warning_box(ui: &mut egui::Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            26,
        ))
        .stroke(Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 120),
        ))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("!").color(color).strong().size(14.0));
                ui.label(RichText::new(text).color(theme::TEXT).size(11.5));
            });
        });
}

fn keyword_confirm(ui: &mut egui::Ui, keyword: &str, input: &mut String, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Gõ").color(theme::MUTED).size(12.0));
        ui.label(
            RichText::new(keyword)
                .monospace()
                .strong()
                .color(color)
                .size(12.5),
        );
        ui.label(
            RichText::new("để xác nhận thao tác:")
                .color(theme::MUTED)
                .size(12.0),
        );
    });
    ui.add(
        egui::TextEdit::singleline(input)
            .hint_text(keyword)
            .desired_width(300.0)
            .font(egui::TextStyle::Monospace),
    );
}

fn buttons(
    ui: &mut egui::Ui,
    cancel_text: &str,
    ok_text: &str,
    ok_fill: Color32,
    ok_enabled: bool,
    out: &mut Outcome,
    build: impl FnOnce() -> Outcome,
) {
    ui.separator();
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 34.0),
        Layout::right_to_left(Align::Center),
        |ui| {
            let ok = ui.add_enabled(
                ok_enabled,
                egui::Button::new(RichText::new(ok_text).strong().color(Color32::WHITE))
                    .fill(ok_fill)
                    .min_size(Vec2::new(110.0, 32.0)),
            );
            if ok.clicked() {
                *out = build();
            }
            if ui.button(cancel_text).clicked() {
                *out = Outcome::Close;
            }
        },
    );
}

// ---------------------------------------------------------------------------
// Tương thích: các hàm model dùng chung
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn _size_helpers() {
    let _ = model::align_up(1, 1);
    let _ = model::short_bytes(1);
}
