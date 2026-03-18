// ─────────────────────────────────────────────────────────────────────────────
//  teuxdeux-rs – Local-first weekly to-do  ·  Rust + Slint
// ─────────────────────────────────────────────────────────────────────────────
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tray_win;

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, Weekday};
use notify_rust::Notification;
use serde::{Deserialize, Serialize};
use slint::{Model, ModelRc, SharedString, TimerMode, VecModel};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs, rc::Rc,
    path::PathBuf,
    time::SystemTime,
};
use uuid::Uuid;

slint::include_modules!();

// ─── Data structures ─────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct TaskRecord {
    id:       String,
    text:     String,
    done:     bool,
    /// "DD.MM.YYYY"
    #[serde(default)] due_date: Option<String>,
    /// "HH:MM"
    #[serde(default)] due_time: Option<String>,
    /// Dateiname (ohne Pfad) in images/-Unterordner neben data.json
    #[serde(default)] image_filename: Option<String>,
    /// Hex-Farbe z.B. "#e74c3c", None = keine Farbe
    #[serde(default)] color_hex: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct SomedayListRecord {
    id:    String,
    title: String,
    tasks: Vec<TaskRecord>,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
struct AppData {
    day_tasks:     HashMap<String, Vec<TaskRecord>>,
    someday_lists: Vec<SomedayListRecord>,
}

// ─── Config ──────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Default)]
struct AppConfig {
    data_file: Option<PathBuf>,
    /// Unix-Timestamp (Sekunden) wann wir data.json zuletzt geschrieben haben.
    /// None = noch nie geschrieben oder ältere Config-Version.
    #[serde(default)]
    last_saved_secs: Option<u64>,
    /// Akzentfarbe als Hex-String, z.B. "#2563eb". None = Standard (#1c1c1c).
    #[serde(default)]
    accent_color: Option<String>,
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("teuxdeux-rs")
        .join("config.json")
}

fn load_config() -> AppConfig {
    let p = config_path();
    if p.exists() {
        serde_json::from_str(&fs::read_to_string(&p).unwrap_or_default())
            .unwrap_or_default()
    } else { AppConfig::default() }
}

fn save_config(cfg: &AppConfig) {
    let p = config_path();
    let _ = fs::create_dir_all(p.parent().unwrap());
    let _ = fs::write(&p, serde_json::to_string_pretty(cfg).unwrap());
}

// ─── I/O ─────────────────────────────────────────────────────────────────────

fn default_data_file() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("teuxdeux-rs")
        .join("data.json")
}

fn load_data(path: &PathBuf) -> AppData {
    if path.exists() {
        serde_json::from_str(&fs::read_to_string(path).unwrap_or_default())
            .unwrap_or_default()
    } else { AppData::default() }
}

fn save_data(data: &AppData, path: &PathBuf) {
    let _ = fs::create_dir_all(path.parent().unwrap());
    let _ = fs::write(path, serde_json::to_string_pretty(data).unwrap());
}

/// Speichert data.json, merkt mtime, speichert Undo-History.
/// Alle drei Schritte sind unabhängig — kein Schritt kann einen anderen blockieren.
fn save_and_record(
    data: &AppData,
    path: &PathBuf,
    cfg: &Rc<RefCell<AppConfig>>,
    stack: &UndoStack,
    last_own_write: &Rc<RefCell<Option<u64>>>,
) {
    save_data(data, path);
    let mtime = file_mtime_secs(path);
    cfg.borrow_mut().last_saved_secs = mtime;
    *last_own_write.borrow_mut() = mtime;
    save_history(stack, path);
    delete_orphaned_images(data, stack, path);
}

/// Liefert die Modify-Zeit einer Datei als Unix-Sekunden, oder None.
fn file_mtime_secs(path: &PathBuf) -> Option<u64> {
    fs::metadata(path).ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

/// Pfad zur Undo-History-Datei (neben der data.json).
fn history_file(data_path: &PathBuf) -> PathBuf {
    data_path.with_extension("history.json")
}

/// Undo-History speichern.
fn save_history(stack: &UndoStack, data_path: &PathBuf) {
    let path = history_file(data_path);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(stack) {
        let _ = fs::write(path, json);
    }
}

/// Undo-History laden. Gibt leeren Stack zurück wenn nicht vorhanden.
fn load_history(data_path: &PathBuf) -> UndoStack {
    let path = history_file(data_path);
    if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path).unwrap_or_default())
            .unwrap_or_else(|_| UndoStack::new())
    } else {
        UndoStack::new()
    }
}

// ─── Date/time helpers ───────────────────────────────────────────────────────

fn start_date(week_offset: i32) -> NaiveDate {
    Local::now().date_naive() + Duration::days(week_offset as i64 * 7)
}

fn kw_label(start: NaiveDate) -> String {
    let end = start + Duration::days(6);
    let start_kw = start.iso_week().week();
    let end_kw = end.iso_week().week();
    if start_kw == end_kw {
        format!("KW{}", start_kw)
    } else {
        format!("KW{} | KW{}", start_kw, end_kw)
    }
}

fn week_label(start: NaiveDate) -> String {
    let end = start + Duration::days(6);
    let months = ["Jan","Feb","Mär","Apr","Mai","Jun","Jul","Aug","Sep","Okt","Nov","Dez"];
    let ms = months[start.month0() as usize];
    let me = months[end.month0() as usize];
    if start.month() == end.month() {
        format!("{}. – {}. {} {}", start.day(), end.day(), me, end.year())
    } else {
        format!("{}. {} – {}. {} {}", start.day(), ms, end.day(), me, end.year())
    }
}

fn day_short(wd: Weekday) -> &'static str {
    match wd {
        Weekday::Mon => "MO", Weekday::Tue => "DI", Weekday::Wed => "MI",
        Weekday::Thu => "DO", Weekday::Fri => "FR",
        Weekday::Sat => "SA", Weekday::Sun => "SO",
    }
}

fn date_label(date: NaiveDate) -> String {
    let months = ["Jan","Feb","Mär","Apr","Mai","Jun","Jul","Aug","Sep","Okt","Nov","Dez"];
    format!("{}. {}", date.day(), months[date.month0() as usize])
}

fn date_key(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Parse "DD.MM.YYYY" → NaiveDate
fn parse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    // Try DD.MM.YYYY
    if let Some(d) = NaiveDate::parse_from_str(s, "%d.%m.%Y").ok() { return Some(d); }
    // Try YYYY-MM-DD
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// Parse "HH:MM" → (hour, minute)
fn parse_time(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let mut parts = s.splitn(2, ':');
    let h: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    if h < 24 && m < 60 { Some((h, m)) } else { None }
}

/// Format a TaskRecord's due date for display in the UI.
fn due_label(t: &TaskRecord) -> String {
    let Some(ds) = &t.due_date else { return String::new(); };
    let Some(date) = parse_date(ds) else { return format!("📅 {}", ds); };
    let months = ["Jan","Feb","Mär","Apr","Mai","Jun","Jul","Aug","Sep","Okt","Nov","Dez"];
    format!("{}. {}", date.day(), months[date.month0() as usize])
}

fn due_time_label(t: &TaskRecord) -> String {
    t.due_time.as_deref().unwrap_or("").to_string()
}

fn due_datetime(t: &TaskRecord) -> Option<NaiveDateTime> {
    let date = parse_date(t.due_date.as_deref()?)?;
    let (h, m) = t.due_time.as_deref()
        .and_then(parse_time)
        .unwrap_or((0, 0));
    date.and_hms_opt(h, m, 0)
}

fn due_flags(t: &TaskRecord) -> (bool, bool) {
    let now = Local::now().naive_local();
    let today = now.date();
    let Some(dt) = due_datetime(t) else { return (false, false); };
    let overdue = dt < now;
    let is_today = !overdue && dt.date() == today;
    (overdue, is_today)
}

// ─── Slint model builders ─────────────────────────────────────────────────────

fn slint_task(t: &TaskRecord) -> Task {
    let label = due_label(t);
    let (overdue, today) = due_flags(t);
    Task {
        id:             SharedString::from(t.id.as_str()),
        text:           SharedString::from(t.text.as_str()),
        done:           t.done,
        due_label:      SharedString::from(label.as_str()),
        due_time_label: SharedString::from(due_time_label(t).as_str()),
        due_overdue:    overdue,
        due_today:      today,
        has_image:      t.image_filename.is_some(),
        color_hex:      SharedString::from(t.color_hex.as_deref().unwrap_or("")),
    }
}

fn build_days(monday: NaiveDate, data: &AppData, hide_done: bool) -> ModelRc<DayData> {
    let today = Local::now().date_naive();
    let days: Vec<DayData> = (0..7).map(|i| {
        let date = monday + Duration::days(i);
        let key  = date_key(date);
        let raw_tasks = data.day_tasks.get(&key);
        let undone_count = raw_tasks.map(|v| v.iter().filter(|t| !t.done).count()).unwrap_or(0);
        let tasks: Vec<Task> = raw_tasks
            .map(|v| v.iter().filter(|t| !(hide_done && t.done)).map(slint_task).collect())
            .unwrap_or_default();
        DayData {
            day_name:    SharedString::from(day_short(date.weekday())),
            date_label:  SharedString::from(date_label(date).as_str()),
            date_str:    SharedString::from(key.as_str()),
            is_today:    date == today,
            is_past:     date < today,
            tasks:       ModelRc::new(VecModel::from(tasks)),
            undone_count: undone_count as i32,
        }
    }).collect();
    ModelRc::new(VecModel::from(days))
}

fn build_someday(data: &AppData, hide_done: bool) -> ModelRc<SomedayList> {
    let lists: Vec<SomedayList> = data.someday_lists.iter().map(|sl| {
        let tasks: Vec<Task> = sl.tasks.iter()
            .filter(|t| !(hide_done && t.done))
            .map(slint_task).collect();
        SomedayList {
            id:    SharedString::from(sl.id.as_str()),
            title: SharedString::from(sl.title.as_str()),
            tasks: ModelRc::new(VecModel::from(tasks)),
        }
    }).collect();
    ModelRc::new(VecModel::from(lists))
}

// ─── Business logic ───────────────────────────────────────────────────────────

/// Klemmt den Ziel-Index so, dass erledigte und unerledigt Tasks nicht gemischt werden.
/// Unerledigt-Tasks dürfen nur im Block vor dem ersten erledigten Task landen,
/// erledigte Tasks nur danach.
fn clamp_to_zone(v: &[TaskRecord], task_done: bool, raw: usize) -> usize {
    let first_done = v.iter().position(|t| t.done).unwrap_or(v.len());
    if task_done {
        // Erledigt: darf nur ab first_done eingefügt werden
        raw.max(first_done).min(v.len())
    } else {
        // Unerledigt: darf nur bis first_done eingefügt werden
        raw.min(first_done).min(v.len())
    }
}

fn move_task_between_days(
    data: &mut AppData, start: NaiveDate,
    source_col: usize, source_task_idx: usize, task_id: &str,
    target_col: usize, target_idx: i32,
) -> bool {
    let src = date_key(start + Duration::days(source_col as i64));
    let tgt = date_key(start + Duration::days(target_col as i64));
    if src == tgt {
        let v = data.day_tasks.entry(src).or_default();
        if let Some(from) = v.iter().position(|t| t.id == task_id) {
            let task = v.remove(from);
            let mut raw = if target_idx < 0 { v.len() } else { (target_idx as usize).min(v.len()) };
            // If we move a task downward (target index after source), remove() shrinks the list
            // and the target index needs to be shifted left by 1.
            if target_idx >= 0 && (target_idx as usize) > source_task_idx {
                raw = raw.saturating_sub(1);
            }
            let to = clamp_to_zone(v, task.done, raw);
            v.insert(to, task);
        }
        return true;
    }
    let task = {
        let v = data.day_tasks.entry(src).or_default();
        v.iter().position(|t| t.id == task_id).map(|i| v.remove(i))
    };
    if let Some(t) = task {
        let v = data.day_tasks.entry(tgt).or_default();
        let raw = if target_idx < 0 { v.len() } else { (target_idx as usize).min(v.len()) };
        let to = clamp_to_zone(v, t.done, raw);
        v.insert(to, t);
        true
    } else { false }
}

/// Verschiebt alle unerledigten Tasks vergangener Tage auf heute.
/// Wird beim Start und bei Mitternacht aufgerufen.
fn rollover_undone(data: &mut AppData, today: NaiveDate) {
    let today_key = date_key(today);
    // Schlüssel vergangener Tage mit unerledigten Tasks sammeln
    let past_keys: Vec<String> = data.day_tasks.keys()
        .filter(|k| **k < today_key)
        .cloned()
        .collect();
    for key in past_keys {
        if let Some(v) = data.day_tasks.get_mut(&key) {
            let mut undone = Vec::new();
            v.retain(|t| {
                if t.done {
                    true
                } else {
                    undone.push(t.clone());
                    false
                }
            });
            if !undone.is_empty() {
                let today_tasks = data.day_tasks.entry(today_key.clone()).or_default();
                // Unerledigt vor alle erledigten Tasks einfügen (Zone respektieren)
                let first_done = today_tasks.iter().position(|t| t.done).unwrap_or(today_tasks.len());
                for (i, t) in undone.into_iter().enumerate() {
                    today_tasks.insert(first_done + i, t);
                }
            }
        }
    }
}


/// Walk all tasks and fire OS notifications for overdue items not yet notified.
/// Registriert die App als WinRT-Notification-Sender (Windows only).
/// Schreibt einen einfachen Registry-Eintrag unter HKCU so dass Windows
/// den Programm-Namen "teuxdeux" und das Exe-Icon in Benachrichtigungen zeigt.
/// Stellt sicher dass nur eine Instanz läuft (Windows).
/// Gibt ein opakes Handle zurück das am Leben bleiben muss, solange die App läuft.
#[cfg(windows)]
fn ensure_single_instance() -> Option<winapi::um::winnt::HANDLE> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::synchapi::CreateMutexW;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::shared::winerror::ERROR_ALREADY_EXISTS;

    let name: Vec<u16> = OsStr::new("Local\\TuDu_SingleInstance")
        .encode_wide().chain(std::iter::once(0)).collect();
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // Bereits eine Instanz aktiv – beenden
        std::process::exit(0);
    }
    if handle.is_null() { None } else { Some(handle) }
}
#[cfg(not(windows))]
fn ensure_single_instance() -> Option<()> { None }

#[cfg(windows)]
fn register_windows_notifications() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use winapi::um::winreg::{RegCreateKeyExW, RegSetValueExW, HKEY_CURRENT_USER};
    use winapi::um::winnt::{KEY_WRITE, REG_SZ, REG_OPTION_NON_VOLATILE};
    
    let app_id = "TuDu";
    let key_path = format!(
        "SOFTWARE\\Classes\\AppUserModelId\\{}",
        app_id
    );

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }
    fn set_sz(hkey: winapi::shared::minwindef::HKEY, name: &str, value: &str) {
        let name_w  = to_wide(name);
        let value_w = to_wide(value);
        unsafe {
            RegSetValueExW(
                hkey,
                name_w.as_ptr(),
                0, REG_SZ,
                value_w.as_ptr() as *const u8,
                (value_w.len() * 2) as u32,
            );
        }
    }

    let key_w = to_wide(&key_path);
    let mut hkey: winapi::shared::minwindef::HKEY = std::ptr::null_mut();
    let mut disposition = 0u32;
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key_w.as_ptr(),
            0, std::ptr::null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null_mut(),
            &mut hkey,
            &mut disposition,
        )
    };
    if result == 0 { // ERROR_SUCCESS == 0
        set_sz(hkey, "DisplayName", "TuDu");
        // IconUri: Pfad zur exe selbst (hat das Icon eingebettet via winresource)
        if let Ok(exe) = std::env::current_exe() {
            let icon_uri = exe.to_string_lossy().to_string();
            set_sz(hkey, "IconUri", &icon_uri);
        }
        unsafe { winapi::um::winreg::RegCloseKey(hkey); }
    }
}
#[cfg(not(windows))]
fn register_windows_notifications() {}

fn parse_hex_color(hex: &str) -> Option<slint::Color> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 { return None; }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(slint::Color::from_rgb_u8(r, g, b))
}

fn apply_accent(ui: &AppWindow, hex: &str) {
    if let Some(c) = parse_hex_color(hex) {
        ui.global::<Theme>().set_accent(c);
        ui.global::<Theme>().set_today_bg(c);
    }
}

fn check_notifications(data: &AppData, notified: &mut HashSet<String>) {
    let now = Local::now().naive_local();

    let mut check = |t: &TaskRecord| {
        if t.done || notified.contains(&t.id) { return; }
        if let Some(dt) = due_datetime(t) {
            if dt <= now {
                notified.insert(t.id.clone());
                let date_str = t.due_date.as_deref()
                    .and_then(|ds| parse_date(ds))
                    .map(|d| {
                        let months = ["Jan","Feb","Mär","Apr","Mai","Jun",
                                      "Jul","Aug","Sep","Okt","Nov","Dez"];
                        format!("{}. {}", d.day(), months[d.month0() as usize])
                    })
                    .unwrap_or_default();
                let body = if let Some(ts) = &t.due_time {
                    format!("\"{}\" war am {} um {} Uhr fällig.", t.text, date_str, ts)
                } else {
                    format!("\"{}\" war am {} fällig.", t.text, date_str)
                };
                let _ = Notification::new()
                    .summary("TuDu – Fälligkeit")
                    .body(&body)
                    .app_id("TuDu")   // entspricht dem Registry-Key oben
                    .show();
            }
        }
    };

    for tasks in data.day_tasks.values() {
        for t in tasks { check(t); }
    }
    for list in &data.someday_lists {
        for t in &list.tasks { check(t); }
    }
}

// ─── Load icon RGBA for tray ─────────────────────────────────────────────────

fn load_icon_rgba() -> Option<(Vec<u8>, u32, u32)> {
    use image::GenericImageView;
    const PNG: &[u8] = include_bytes!("../assets/icon.png");
    let img = image::load_from_memory(PNG).ok()?;
    let (w, h) = img.dimensions();
    Some((img.into_rgba8().into_raw(), w, h))
}

// ─── Main ────────────────────────────────────────────────────────────────────

// ─── Undo/Redo ────────────────────────────────────────────────────────────────

const UNDO_LIMIT: usize = 10;

#[derive(Serialize, Deserialize)]
struct UndoStack {
    past:   Vec<AppData>,   // älteste … neueste Vorgänger
    future: Vec<AppData>,   // neueste … älteste Nachfolger
}

impl UndoStack {
    fn new() -> Self { Self { past: vec![], future: vec![] } }

    /// Vor jeder Mutation aufrufen: Zustand sichern, Redo löschen.
    fn push(&mut self, snapshot: AppData) {
        self.future.clear();
        self.past.push(snapshot);
        if self.past.len() > UNDO_LIMIT {
            self.past.remove(0);
        }
    }

    /// Gibt den Zustand vor der letzten Mutation zurück (undo).
    fn undo(&mut self, current: AppData) -> Option<AppData> {
        let prev = self.past.pop()?;
        self.future.push(current);
        Some(prev)
    }

    /// Gibt den Zustand nach dem letzten Undo zurück (redo).
    fn redo(&mut self, current: AppData) -> Option<AppData> {
        let next = self.future.pop()?;
        self.past.push(current);
        Some(next)
    }
}


// ─── Image attachment helpers ────────────────────────────────────────────────

fn images_dir(data_path: &PathBuf) -> PathBuf {
    data_path.parent()
        .unwrap_or(std::path::Path::new("."))
        .join("images")
}

/// Kopiert ein Bild in den images/-Ordner, gibt den neuen Dateinamen zurück.
fn copy_image_to_store(src: &std::path::Path, data_path: &PathBuf) -> Option<String> {
    let ext = src.extension()?.to_str()?.to_lowercase();
    let canonical_ext = match ext.as_str() {
        "jpg" | "jpeg" => "jpg",
        "png"          => "png",
        _              => return None,
    };
    let dir = images_dir(data_path);
    let _ = fs::create_dir_all(&dir);
    let filename = format!("{}.{}", Uuid::new_v4(), canonical_ext);
    let dest = dir.join(&filename);
    fs::copy(src, &dest).ok()?;
    Some(filename)
}

/// Sammelt alle in `data` referenzierten Bilddateinamen.
fn collect_referenced_images(data: &AppData) -> HashSet<String> {
    let mut refs = HashSet::new();
    for tasks in data.day_tasks.values() {
        for t in tasks {
            if let Some(f) = &t.image_filename { refs.insert(f.clone()); }
        }
    }
    for list in &data.someday_lists {
        for t in &list.tasks {
            if let Some(f) = &t.image_filename { refs.insert(f.clone()); }
        }
    }
    refs
}

/// Löscht Bilder aus images/, die in keiner Undo-Snapshot mehr referenziert sind.
fn delete_orphaned_images(current: &AppData, stack: &UndoStack, data_path: &PathBuf) {
    let dir = images_dir(data_path);
    if !dir.exists() { return; }
    let mut referenced = collect_referenced_images(current);
    for snap in &stack.past   { referenced.extend(collect_referenced_images(snap)); }
    for snap in &stack.future { referenced.extend(collect_referenced_images(snap)); }
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if !referenced.contains(name) {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    // Nur eine Instanz erlauben
    let _mutex = ensure_single_instance();

    // Windows: App als Notification-Sender registrieren (Name + Icon)
    register_windows_notifications();

    std::env::set_var("LANG", "de_DE.UTF-8");

    // On Linux, tray-icon needs GTK initialised before Slint.
    #[cfg(target_os = "linux")]
    {
        // gtk is a transitive dep of tray-icon on Linux; init it.
        // If gtk feature is not available this is a no-op.
        let _ = std::process::Command::new("true").status(); // dummy – gtk init happens below
    }

    // ── Load config & data ───────────────────────────────────────────────────
    let cfg: Rc<RefCell<AppConfig>> = Rc::new(RefCell::new(load_config()));
    let data_file: Rc<RefCell<PathBuf>> = Rc::new(RefCell::new(
        cfg.borrow().data_file.clone().unwrap_or_else(default_data_file)
    ));
    let app_data: Rc<RefCell<AppData>> = Rc::new(RefCell::new(
        load_data(&data_file.borrow())
    ));

    {
        let mut d = app_data.borrow_mut();
        if d.someday_lists.is_empty() {
            d.someday_lists.push(SomedayListRecord {
                id: Uuid::new_v4().to_string(), title: "Someday".into(), tasks: vec![],
            });
        }
        // Unerledigt vergangener Tage → heute verschieben
        rollover_undone(&mut d, Local::now().date_naive());
        save_data(&d, &data_file.borrow());
        // mtime direkt in cfg eintragen (cfg ist hier noch kein Rc)
        cfg.borrow_mut().last_saved_secs = file_mtime_secs(&data_file.borrow());
        save_config(&cfg.borrow());
    }

    // Offset in weeks from today (0 = window starting today, 1 = next 7 days, -1 = previous 7 days)
    let week_offset: Rc<RefCell<i32>> = Rc::new(RefCell::new(0));
    let hide_done:  Rc<RefCell<bool>>          = Rc::new(RefCell::new(false));
    let notified:   Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));

    // ── Undo-History laden + externe Änderung prüfen ─────────────────────────
    let extern_changed_msg: Option<String> = {
        let data_path = data_file.borrow().clone();
        let data_mtime = file_mtime_secs(&data_path);
        let cfg_saved  = cfg.borrow().last_saved_secs;
        match (data_mtime, cfg_saved) {
            (Some(dm), Some(cs)) if dm > cs => {
                // data.json ist neuer als unser letzter Schreibvorgang → extern geändert
                Some(format!(
                    "data.json wurde extern geändert – Undo-History wurde zurückgesetzt."
                ))
            }
            _ => None,
        }
    };

    let undo_stack: Rc<RefCell<UndoStack>> = Rc::new(RefCell::new(
        if extern_changed_msg.is_some() {
            // Extern geändert → History ungültig, neu beginnen
            let _ = fs::remove_file(history_file(&data_file.borrow()));
            UndoStack::new()
        } else {
            load_history(&data_file.borrow())
        }
    ));

    // In-memory mtime nach eigenem Schreiben — verhindert Self-Trigger beim Polling.
    let last_own_write: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(
        file_mtime_secs(&data_file.borrow())
    ));

    // ── Create UI ─────────────────────────────────────────────────────────────
    let ui = AppWindow::new()?;
    // Muss NACH dem ersten Component-Aufruf stehen (Slint-Anforderung)
    slint::select_bundled_translation("de").ok();

    // Which task is being edited for due-date: (col_idx, list_idx, task_id)
    // col_idx >= 0 → day task; col_idx == -1 → someday task at list_idx
    let editing: Rc<RefCell<Option<(i32, i32, String)>>> = Rc::new(RefCell::new(None));

    // ── Initial render ───────────────────────────────────────────────────────
    {
        let start = start_date(*week_offset.borrow());
        let d     = app_data.borrow();
        let hd    = *hide_done.borrow();
        let fp    = data_file.borrow();
        {
            let us = undo_stack.borrow();
            ui.set_can_undo(!us.past.is_empty());
            ui.set_can_redo(!us.future.is_empty());
        }
        ui.set_week_label(SharedString::from(week_label(start).as_str()));
        ui.set_kw_label(SharedString::from(kw_label(start).as_str()));
        ui.set_days(build_days(start, &d, hd));
        ui.set_someday_lists(build_someday(&d, hd));
        ui.set_data_path_label(SharedString::from(fp.parent().unwrap_or(fp.as_path()).to_string_lossy().as_ref()));
        // Gespeicherte Akzentfarbe anwenden
        if let Some(ref hex) = cfg.borrow().accent_color.clone() {
            ui.set_accent_hex(SharedString::from(hex.as_str()));
            apply_accent(&ui, hex);
        }
        if let Some(ref msg) = extern_changed_msg {
            ui.set_extern_changed_info(SharedString::from(msg.as_str()));
        }
    }

    // ── Snapshot-Closure (vor jeder Mutation aufrufen) ───────────────────────
    // snapshot(data): nur Undo-Stack pushen – kein I/O, kann nicht paniken.
    // save_history wird von jedem Callback NACH save_and_record separat aufgerufen.
    let snapshot = {
        let us = Rc::clone(&undo_stack);
        move |data: AppData| {
            us.borrow_mut().push(data);
        }
    };

    // ── Refresh closure ──────────────────────────────────────────────────────
    let refresh = {
        let ui_w = ui.as_weak();
        let off_r = Rc::clone(&week_offset);
        let d_r  = Rc::clone(&app_data);
        let hd_r = Rc::clone(&hide_done);
        let fp_r = Rc::clone(&data_file);
        let us_r = Rc::clone(&undo_stack);
        move || {
            let ui = ui_w.unwrap();
            let start = start_date(*off_r.borrow());
            let d  = d_r.borrow();
            let hd = *hd_r.borrow();
            let fp = fp_r.borrow();
            let us = us_r.borrow();
            ui.set_can_undo(!us.past.is_empty());
            ui.set_can_redo(!us.future.is_empty());
            drop(us);
            ui.set_week_label(SharedString::from(week_label(start).as_str()));
            ui.set_kw_label(SharedString::from(kw_label(start).as_str()));
            ui.set_days(build_days(start, &d, hd));
            ui.set_someday_lists(build_someday(&d, hd));
            ui.set_hide_done(hd);
            ui.set_data_path_label(SharedString::from(fp.parent().unwrap_or(fp.as_path()).to_string_lossy().as_ref()));
        }
    };

    // ════════════════════════════════════════════════════════════════════════
    //  Callbacks
    // ════════════════════════════════════════════════════════════════════════

    // navigate-week (shifts the 7-day window by full weeks)
    { let off_r = Rc::clone(&week_offset); let rf = refresh.clone();
      ui.on_navigate_week(move |d| { *off_r.borrow_mut() += d; rf(); }); }

    // navigate-today (reset to window starting today)
    { let off_r = Rc::clone(&week_offset); let rf = refresh.clone();
      ui.on_navigate_today(move || { *off_r.borrow_mut() = 0; rf(); }); }

    // add-task
    { let off_r = Rc::clone(&week_offset); let d_r = Rc::clone(&app_data);
      let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone(); let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_add_task(move |col, text| {
        sn(d_r.borrow().clone());
                let text = text.trim().to_string(); if text.is_empty() { return; }
        let start = start_date(*off_r.borrow());
        let date = start + Duration::days(col as i64);
        let mut d = d_r.borrow_mut();
        d.day_tasks.entry(date_key(date)).or_default()
            .push(TaskRecord { id: Uuid::new_v4().to_string(), text, done: false, ..Default::default() });
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }

    // toggle-task
    { let off_r = Rc::clone(&week_offset); let d_r = Rc::clone(&app_data);
      let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_toggle_task(move |col, id| {
        sn(d_r.borrow().clone());
        let start = start_date(*off_r.borrow());
        let date  = start + Duration::days(col as i64);
        let today = Local::now().date_naive();
        let mut d = d_r.borrow_mut();
        // Task togglen
        let became_undone = {
            let key = date_key(date);
            if let Some(v) = d.day_tasks.get_mut(&key) {
                if let Some(t) = v.iter_mut().find(|t| t.id == id.as_str()) {
                    t.done = !t.done;
                    !t.done // true wenn gerade auf unerledigt gesetzt
                } else { false }
            } else { false }
        };
        // Wenn in Vergangenheit auf unerledigt gesetzt → heute verschieben
        if became_undone && date < today {
            let key = date_key(date);
            let task = d.day_tasks.get_mut(&key)
                .and_then(|v| v.iter().position(|t| t.id == id.as_str()).map(|i| v.remove(i)));
            if let Some(t) = task {
                let tv = d.day_tasks.entry(date_key(today)).or_default();
                let first_done = tv.iter().position(|t| t.done).unwrap_or(tv.len());
                tv.insert(first_done, t);
            }
        } else {
            // Partition in gleicher Liste
            let key = date_key(date);
            if let Some(v) = d.day_tasks.get_mut(&key) {
                let (undone, done): (Vec<_>, Vec<_>) = v.drain(..).partition(|t| !t.done);
                v.extend(undone); v.extend(done);
            }
        }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }

    // delete-task
    { let off_r = Rc::clone(&week_offset); let d_r = Rc::clone(&app_data);
      let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_delete_task(move |col, id| {
        sn(d_r.borrow().clone());
                let start = start_date(*off_r.borrow());
        let date = start + Duration::days(col as i64);
        let mut d = d_r.borrow_mut();
        if let Some(v) = d.day_tasks.get_mut(&date_key(date)) { v.retain(|t| t.id != id.as_str()); }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }

    // task-drag-dropped (day → day or day → someday)
    { let ui_w = ui.as_weak(); let off_r = Rc::clone(&week_offset);
      let d_r = Rc::clone(&app_data); let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_task_drag_dropped(move || {
        let ui = ui_w.unwrap();
        let drag       = ui.global::<DragState>();
        let src_col    = drag.get_source_col() as usize;
        let task_id    = drag.get_task_id().to_string();
        let tgt_task   = drag.get_target_task_idx();
        let someday_f  = ui.get_drag_target_someday_idx_f();
        let tgt_col    = (ui.get_drag_target_col_f() as i32).clamp(0, 6) as usize;
        drag.set_active(false); drag.set_task_id(SharedString::default());
        sn(d_r.borrow().clone());
                let mut d = d_r.borrow_mut();
        let start = start_date(*off_r.borrow());
        if someday_f >= 0.0 {
            // day → someday: an Zone-Grenze einfügen (erledigt/unerledigt getrennt)
            let list_idx = (someday_f as usize).min(d.someday_lists.len().saturating_sub(1));
            let src_key  = date_key(start + Duration::days(src_col as i64));
            let task = d.day_tasks.get_mut(&src_key)
                .and_then(|v| v.iter().position(|t| t.id == task_id).map(|i| v.remove(i)));
            if let Some(t) = task {
                if let Some(list) = d.someday_lists.get_mut(list_idx) {
                    let to = clamp_to_zone(&list.tasks, t.done, list.tasks.len());
                    list.tasks.insert(to, t);
                }
                save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r);
            }
        } else if src_col <= 6 {
            // day → day (auch Reorder in gleicher Spalte)
            let tgt_date = start + Duration::days(tgt_col as i64);
            let today    = Local::now().date_naive();
            if tgt_date >= today || tgt_col == src_col {
                let src_task_idx = drag.get_source_task_idx() as usize;
                if move_task_between_days(&mut d, start, src_col, src_task_idx, &task_id, tgt_col, tgt_task) {
                    save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r);
                }
            }
        }
        drop(d); rf();
    }); }

    // someday-task-drag-dropped
    { let ui_w = ui.as_weak(); let off_r = Rc::clone(&week_offset);
      let d_r = Rc::clone(&app_data); let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_someday_task_drag_dropped(move || {
        sn(d_r.borrow().clone());
        let ui       = ui_w.unwrap();
        let drag     = ui.global::<DragState>();
        let src_lst  = drag.get_source_list_idx() as usize;
        let task_id  = drag.get_task_id().to_string();
        let tgt_task = drag.get_target_task_idx();
        let someday_f = ui.get_drag_target_someday_idx_f();
        drag.set_active(false); drag.set_task_id(SharedString::default());

        if someday_f >= 0.0 {
            // someday → someday (gleiche oder andere Liste)
            let tgt_lst = (someday_f as usize).min({
                let d = d_r.borrow();
                d.someday_lists.len().saturating_sub(1)
            });
            let task = {
                let mut d = d_r.borrow_mut();
                d.someday_lists.get_mut(src_lst)
                    .and_then(|l| l.tasks.iter().position(|t| t.id == task_id).map(|i| l.tasks.remove(i)))
            };
            if let Some(t) = task {
                let mut d = d_r.borrow_mut();
                if let Some(list) = d.someday_lists.get_mut(tgt_lst) {
                    // Gleiche Liste: tgt_task nutzen; andere Liste: an Zone-Grenze
                    let raw = if src_lst == tgt_lst && tgt_task >= 0 {
                        (tgt_task as usize).min(list.tasks.len())
                    } else {
                        list.tasks.len() // Cross-list: Zone-Grenze (clamp übernimmt)
                    };
                    let to = clamp_to_zone(&list.tasks, t.done, raw);
                    list.tasks.insert(to, t);
                }
                save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r);
            }
        } else {
            // someday → day
            let tgt_col = (ui.get_drag_target_col_f() as i32).clamp(0, 6) as usize;
            let today   = Local::now().date_naive();
            let start   = start_date(*off_r.borrow());
            let tgt_date = start + Duration::days(tgt_col as i64);
            if tgt_date >= today {
                let task = {
                    let mut d = d_r.borrow_mut();
                    d.someday_lists.get_mut(src_lst)
                        .and_then(|l| l.tasks.iter().position(|t| t.id == task_id).map(|i| l.tasks.remove(i)))
                };
                if let Some(t) = task {
                    let mut d = d_r.borrow_mut();
                    let v   = d.day_tasks.entry(date_key(tgt_date)).or_default();
                    let raw = if tgt_task < 0 { v.len() } else { (tgt_task as usize).min(v.len()) };
                    let to  = clamp_to_zone(v, t.done, raw);
                    v.insert(to, t);
                    save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r);
                }
            }
        }
        rf();
    }); }

    // add/toggle/delete someday tasks
    { let d_r = Rc::clone(&app_data); let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_add_someday_task(move |li, text| {
        sn(d_r.borrow().clone());
                let text = text.trim().to_string(); if text.is_empty() { return; }
        let mut d = d_r.borrow_mut();
        if let Some(l) = d.someday_lists.get_mut(li as usize) {
            l.tasks.push(TaskRecord { id: Uuid::new_v4().to_string(), text, done: false, ..Default::default() });
        }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }
    { let d_r = Rc::clone(&app_data); let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_toggle_someday_task(move |li, id| {
        sn(d_r.borrow().clone());
        let today = Local::now().date_naive();
        let mut d = d_r.borrow_mut();
        // Task togglen und prüfen ob er unerledigt wurde
        let became_undone_task = {
            if let Some(l) = d.someday_lists.get_mut(li as usize) {
                if let Some(t) = l.tasks.iter_mut().find(|t| t.id == id.as_str()) {
                    t.done = !t.done;
                    if !t.done {
                        // Unerledigt → auf heute verschieben (aus Someday entfernen)
                        Some(id.to_string())
                    } else { None }
                } else { None }
            } else { None }
        };
        if let Some(ref task_id) = became_undone_task {
            // Aus Someday entfernen und in heute einfügen
            let task = d.someday_lists.get_mut(li as usize)
                .and_then(|l| l.tasks.iter().position(|t| t.id == task_id.as_str()).map(|i| l.tasks.remove(i)));
            if let Some(t) = task {
                let tv = d.day_tasks.entry(date_key(today)).or_default();
                let first_done = tv.iter().position(|t| t.done).unwrap_or(tv.len());
                tv.insert(first_done, t);
            }
        } else {
            // Nur Partition (done blieb done, unerledigt blieb unerledigt)
            if let Some(l) = d.someday_lists.get_mut(li as usize) {
                let (undone, done): (Vec<_>, Vec<_>) = l.tasks.drain(..).partition(|t| !t.done);
                l.tasks.extend(undone); l.tasks.extend(done);
            }
        }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }
    { let d_r = Rc::clone(&app_data); let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_delete_someday_task(move |li, id| {
        sn(d_r.borrow().clone());
                let mut d = d_r.borrow_mut();
        if let Some(l) = d.someday_lists.get_mut(li as usize) { l.tasks.retain(|t| t.id != id.as_str()); }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }
    { let d_r = Rc::clone(&app_data); let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_add_someday_list(move |title| {
        sn(d_r.borrow().clone());
        let title = { let t = title.trim().to_string(); if t.is_empty() { "Neue Liste".into() } else { t } };
        let mut d = d_r.borrow_mut();
        d.someday_lists.push(SomedayListRecord { id: Uuid::new_v4().to_string(), title, tasks: vec![] });
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }
    { let d_r = Rc::clone(&app_data); let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg);
      let sn = snapshot.clone(); let ui_w = ui.as_weak();
      let us_r = Rc::clone(&undo_stack);
      ui.on_rename_someday_list(move |idx, new_title| {
        let new_title = new_title.trim().to_string();
        if new_title.is_empty() { return; }
        sn(d_r.borrow().clone());
        let mut d = d_r.borrow_mut();
        if let Some(l) = d.someday_lists.get_mut(idx as usize) { l.title = new_title.clone(); }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d);
        // Kein rf() — nur den Titel im bestehenden Modell patchen.
        // rf() würde alle SomedayListCards neu aufbauen und das aktive TextInput zerstören.
        if let Some(ui) = ui_w.upgrade() {
            let lists = ui.get_someday_lists();
            if let Some(mut entry) = lists.row_data(idx as usize) {
                entry.title = SharedString::from(new_title.as_str());
                lists.set_row_data(idx as usize, entry);
            }
            let us = us_r.borrow();
            ui.set_can_undo(!us.past.is_empty());
            ui.set_can_redo(!us.future.is_empty());
        }
    }); }
    { let d_r = Rc::clone(&app_data); let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_delete_someday_list(move |idx| {
        sn(d_r.borrow().clone());
        let mut d = d_r.borrow_mut();
        if (idx as usize) < d.someday_lists.len() {
            d.someday_lists.remove(idx as usize);
        }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }
    { let d_r = Rc::clone(&app_data); let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_reorder_someday_list(move |from: i32, to: i32| {
        let from = from as usize;
        let to   = to   as usize;
        let n    = d_r.borrow().someday_lists.len();
        // Gleiche Position oder logisch keine Änderung (cursor war rechts vom source)
        if from >= n || to > n || from == to { return; }
        if from < to && to == from + 1 { return; }  // Einfügen direkt nach sich selbst
        sn(d_r.borrow().clone());
        let mut d = d_r.borrow_mut();
        let item = d.someday_lists.remove(from);
        let insert_at = if from < to { to - 1 } else { to };
        d.someday_lists.insert(insert_at, item);
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }

    // toggle-hide-done / delete-all-done
    { let hd_r = Rc::clone(&hide_done); let rf = refresh.clone();
      ui.on_toggle_hide_done(move || { let mut hd = hd_r.borrow_mut(); *hd = !*hd; drop(hd); rf(); }); }
    { let d_r = Rc::clone(&app_data); let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_delete_all_done(move || {
        sn(d_r.borrow().clone());
                let mut d = d_r.borrow_mut();
        for v in d.day_tasks.values_mut() { v.retain(|t| !t.done); }
        for l in d.someday_lists.iter_mut() { l.tasks.retain(|t| !t.done); }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }


    // rename-task (col=-1 → someday at list_idx)
    { let d_r = Rc::clone(&app_data); let off_r = Rc::clone(&week_offset);
      let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      let sn = snapshot.clone();
      let us_r = Rc::clone(&undo_stack);
      ui.on_rename_task(move |col, list_idx, id, new_text| {
        let new_text = new_text.trim().to_string();
        if new_text.is_empty() { return; }
        sn(d_r.borrow().clone());
                let mut d = d_r.borrow_mut();
        if col >= 0 {
            let start = start_date(*off_r.borrow());
            let date = start + Duration::days(col as i64);
            if let Some(v) = d.day_tasks.get_mut(&date_key(date)) {
                if let Some(t) = v.iter_mut().find(|t| t.id == id.as_str()) {
                    t.text = new_text;
                }
            }
        } else {
            if let Some(l) = d.someday_lists.get_mut(list_idx as usize) {
                if let Some(t) = l.tasks.iter_mut().find(|t| t.id == id.as_str()) {
                    t.text = new_text;
                }
            }
        }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }

    // change-data-path
    { let cfg_r = Rc::clone(&cfg); let d_r = Rc::clone(&app_data);
      let fp_r  = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let us_r = Rc::clone(&undo_stack); let rf = refresh.clone();
      ui.on_change_data_path(move || {
        let cur = fp_r.borrow().parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
        if let Some(folder) = rfd::FileDialog::new()
                .set_title("Ordner für data.json wählen")
                .set_directory(&cur).pick_folder() {
            let new_file = folder.join("data.json");
            { let d = d_r.borrow(); save_and_record(&d, &new_file, &cfg_r, &us_r.borrow(), &low_r); }
            *fp_r.borrow_mut() = new_file.clone();
            { let mut c = cfg_r.borrow_mut(); c.data_file = Some(new_file.clone()); }
            save_config(&cfg_r.borrow());   // einmalig beim Pfadwechsel
            rf();
        }
    }); }

    // ── set-accent-color ─────────────────────────────────────────────────────
    { let cfg_r = Rc::clone(&cfg); let ui_w = ui.as_weak();
      ui.on_set_accent_color(move |hex| {
        let ui = ui_w.unwrap();
        apply_accent(&ui, hex.as_str());
        let mut c = cfg_r.borrow_mut();
        c.accent_color = Some(hex.to_string());
        save_config(&c);
    }); }

    // ── reset-settings ───────────────────────────────────────────────────────
    { let cfg_r = Rc::clone(&cfg); let d_r = Rc::clone(&app_data);
      let fp_r  = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write);
      let us_r  = Rc::clone(&undo_stack); let rf = refresh.clone();
      let ui_w  = ui.as_weak();
      ui.on_reset_settings(move || {
        let ui = ui_w.unwrap();
        // Akzentfarbe zurücksetzen
        let default_hex = "#1c1c1c";
        apply_accent(&ui, default_hex);
        ui.set_accent_hex(SharedString::from(default_hex));
        // Datenpfad zurücksetzen
        let default_path = default_data_file();
        { let d = d_r.borrow(); save_and_record(&d, &default_path, &cfg_r, &us_r.borrow(), &low_r); }
        *fp_r.borrow_mut() = default_path.clone();
        // Config zurücksetzen (accent + data_file löschen, last_saved behalten)
        {
            let mut c = cfg_r.borrow_mut();
            c.accent_color = None;
            c.data_file    = None;
        }
        save_config(&cfg_r.borrow());
        rf();
    }); }

    // ── set-task-due: öffnet DatePickerPopup (oder Options-Popup) ────────────
    { let d_r    = Rc::clone(&app_data);
      let off_r  = Rc::clone(&week_offset);
      let ui_w   = ui.as_weak();
      let edit_r = Rc::clone(&editing);
      ui.on_set_task_due(move |col, list_idx, task_id| {
        let d     = d_r.borrow();
        let today = Local::now().date_naive();
        let start = start_date(*off_r.borrow());
        let task: Option<TaskRecord> = if col >= 0 {
            let date = start + Duration::days(col as i64);
            d.day_tasks.get(&date_key(date))
                .and_then(|v| v.iter().find(|t| t.id == task_id.as_str())).cloned()
        } else {
            d.someday_lists.get(list_idx as usize)
                .and_then(|l| l.tasks.iter().find(|t| t.id == task_id.as_str())).cloned()
        };
        drop(d);
        *edit_r.borrow_mut() = Some((col, list_idx, task_id.to_string()));
        let ui = ui_w.unwrap();
        // Pre-populate picker with existing date (or today)
        let _has_date = task.as_ref().and_then(|t| t.due_date.as_ref()).is_some();
        if let Some(ref t) = task {
            if let Some(ref ds) = t.due_date {
                let d = parse_date(ds).unwrap_or(today);
                ui.set_picker_day(d.day() as i32);
                ui.set_picker_month(d.month() as i32);
                ui.set_picker_year(d.year());
            } else {
                ui.set_picker_day(today.day() as i32);
                ui.set_picker_month(today.month() as i32);
                ui.set_picker_year(today.year());
            }
            if let Some(ref ts) = t.due_time {
                let (h, m) = parse_time(ts).unwrap_or((9, 0));
                ui.set_picker_hour(h as i32);
                ui.set_picker_minute(m as i32);
            } else {
                ui.set_picker_hour(9);
                ui.set_picker_minute(0);
            }
        } else {
            ui.set_picker_day(today.day() as i32);
            ui.set_picker_month(today.month() as i32);
            ui.set_picker_year(today.year());
            ui.set_picker_hour(9);
            ui.set_picker_minute(0);
        }
        // Popup-Entscheidung (Optionen vs. direkt DatePicker) liegt jetzt in Slint/TaskRow.
        // Rust öffnet immer den DatePicker — entweder direkt (kein Datum) oder
        // nach "Datum ändern" im lokalen TaskRow-Popup.
        ui.invoke_open_date_picker();
    }); }

    // ── Helper: save due date+time for the editing task ──────────────────────
    fn apply_due(
        d_r: &Rc<RefCell<AppData>>,
        start: NaiveDate,
        fp_r: &Rc<RefCell<PathBuf>>,
        cfg_r: &Rc<RefCell<AppConfig>>,
        us_r: &Rc<RefCell<UndoStack>>,
        low_r: &Rc<RefCell<Option<u64>>>,
        edit_r: &Rc<RefCell<Option<(i32, i32, String)>>>,
        due_date: Option<String>,
        due_time: Option<String>,
        rf: &impl Fn(),
    ) {
        if let Some((col, list_idx, task_id)) = edit_r.borrow().clone() {
            let mut d = d_r.borrow_mut();
            let task_opt: Option<&mut TaskRecord> = if col >= 0 {
                let date = start + Duration::days(col as i64);
                d.day_tasks.get_mut(&date_key(date))
                    .and_then(|v| v.iter_mut().find(|t| t.id == task_id))
            } else {
                d.someday_lists.get_mut(list_idx as usize)
                    .and_then(|l| l.tasks.iter_mut().find(|t| t.id == task_id))
            };
            if let Some(t) = task_opt { t.due_date = due_date; t.due_time = due_time; }
            save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
        }
    }

    // ── due-complete: date + time both confirmed ───────────────────────────────
    { let d_r    = Rc::clone(&app_data);
      let off_r  = Rc::clone(&week_offset);
      let fp_r   = Rc::clone(&data_file);
      let cfg_r  = Rc::clone(&cfg);
      let us_r   = Rc::clone(&undo_stack);
      let low_r  = Rc::clone(&last_own_write);
      let edit_r = Rc::clone(&editing);
      let rf     = refresh.clone();
      ui.on_due_complete(move |day, month, year, hour, minute| {
        // Guard: Datum+Uhrzeit darf nicht in der Vergangenheit liegen
        if let Some(chosen_date) = NaiveDate::from_ymd_opt(year, month as u32, day as u32) {
            if let Some(chosen_dt) = chosen_date.and_hms_opt(hour as u32, minute as u32, 0) {
                if chosen_dt < Local::now().naive_local() { return; }
            }
        }
        let date_str = Some(format!("{:02}.{:02}.{}", day, month, year));
        let time_str = Some(format!("{:02}:{:02}", hour, minute));
        let start = start_date(*off_r.borrow());
        apply_due(&d_r, start, &fp_r, &cfg_r, &us_r, &low_r, &edit_r, date_str, time_str, &rf);
    }); }

    // ── due-date-only: date confirmed, time skipped ────────────────────────────
    { let d_r    = Rc::clone(&app_data);
      let off_r  = Rc::clone(&week_offset);
      let fp_r   = Rc::clone(&data_file);
      let cfg_r  = Rc::clone(&cfg);
      let us_r   = Rc::clone(&undo_stack);
      let low_r  = Rc::clone(&last_own_write);
      let edit_r = Rc::clone(&editing);
      let rf     = refresh.clone();
      ui.on_due_date_only(move |day, month, year| {
        if let Some(chosen) = NaiveDate::from_ymd_opt(year, month as u32, day as u32) {
            if chosen < Local::now().date_naive() { return; }
        }
        let date_str = Some(format!("{:02}.{:02}.{}", day, month, year));
        let start = start_date(*off_r.borrow());
        apply_due(&d_r, start, &fp_r, &cfg_r, &us_r, &low_r, &edit_r, date_str, None, &rf);
    }); }

    // ── due-cleared: "Datum entfernen" (über altes Interface, noch für compat) ─
    { let d_r    = Rc::clone(&app_data);
      let off_r  = Rc::clone(&week_offset);
      let fp_r   = Rc::clone(&data_file);
      let cfg_r  = Rc::clone(&cfg);
      let us_r   = Rc::clone(&undo_stack);
      let low_r  = Rc::clone(&last_own_write);
      let edit_r = Rc::clone(&editing);
      let rf     = refresh.clone();
      ui.on_due_cleared(move || {
        let start = start_date(*off_r.borrow());
        apply_due(&d_r, start, &fp_r, &cfg_r, &us_r, &low_r, &edit_r, None, None, &rf);
    }); }

    // ── due-cleared-for: von TaskRow-Popup (col, list_idx, id) ───────────────
    // Setzt editing-Kontext direkt und löscht Datum (kein Umweg über Rust set_task_due).
    { let d_r    = Rc::clone(&app_data);
      let off_r  = Rc::clone(&week_offset);
      let fp_r   = Rc::clone(&data_file);
      let cfg_r  = Rc::clone(&cfg);
      let us_r   = Rc::clone(&undo_stack);
      let low_r  = Rc::clone(&last_own_write);
      let edit_r = Rc::clone(&editing);
      let rf     = refresh.clone();
      ui.on_due_cleared_for(move |col, list_idx, task_id| {
        *edit_r.borrow_mut() = Some((col, list_idx, task_id.to_string()));
        let start = start_date(*off_r.borrow());
        apply_due(&d_r, start, &fp_r, &cfg_r, &us_r, &low_r, &edit_r, None, None, &rf);
    }); }

    // ── attach-image: Dateidialog → Bild kopieren → Task aktualisieren ──────
    { let d_r = Rc::clone(&app_data); let off_r = Rc::clone(&week_offset);
      let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg);
      let sn = snapshot.clone(); let us_r = Rc::clone(&undo_stack); let rf = refresh.clone();
      ui.on_attach_image(move |col, list_idx, id| {
        let Some(src) = rfd::FileDialog::new()
            .set_title("Bild anhängen")
            .add_filter("Bilder", &["jpg", "jpeg", "png"])
            .pick_file() else { return; };
        let Some(filename) = copy_image_to_store(&src, &fp_r.borrow()) else { return; };
        sn(d_r.borrow().clone());
        let mut d = d_r.borrow_mut();
        let start = start_date(*off_r.borrow());
        let task_opt: Option<&mut TaskRecord> = if col >= 0 {
            let date = start + Duration::days(col as i64);
            d.day_tasks.get_mut(&date_key(date))
                .and_then(|v| v.iter_mut().find(|t| t.id == id.as_str()))
        } else {
            d.someday_lists.get_mut(list_idx as usize)
                .and_then(|l| l.tasks.iter_mut().find(|t| t.id == id.as_str()))
        };
        if let Some(t) = task_opt { t.image_filename = Some(filename); }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }

    // ── show-image: Bild laden und Popup öffnen ───────────────────────────────
    { let d_r = Rc::clone(&app_data); let off_r = Rc::clone(&week_offset);
      let fp_r = Rc::clone(&data_file); let ui_w = ui.as_weak();
      ui.on_show_image(move |col, list_idx, id| {
        let d = d_r.borrow();
        let start = start_date(*off_r.borrow());
        let task_opt: Option<&TaskRecord> = if col >= 0 {
            let date = start + Duration::days(col as i64);
            d.day_tasks.get(&date_key(date))
                .and_then(|v| v.iter().find(|t| t.id == id.as_str()))
        } else {
            d.someday_lists.get(list_idx as usize)
                .and_then(|l| l.tasks.iter().find(|t| t.id == id.as_str()))
        };
        if let Some(t) = task_opt {
            if let Some(ref fname) = t.image_filename {
                let path = images_dir(&fp_r.borrow()).join(fname);
                if let Ok(img) = slint::Image::load_from_path(&path) {
                    let ui = ui_w.unwrap();
                    ui.set_popup_image(img);
                    ui.set_popup_image_col(col);
                    ui.set_popup_image_list_idx(list_idx);
                    ui.set_popup_image_task_id(id);
                    ui.invoke_open_image_popup();
                }
            }
        }
    }); }

    // ── delete-image: Bild aus Task entfernen (Datei wird orphan-cleanup gelöscht) ─
    { let d_r = Rc::clone(&app_data); let off_r = Rc::clone(&week_offset);
      let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg);
      let sn = snapshot.clone(); let us_r = Rc::clone(&undo_stack); let rf = refresh.clone();
      ui.on_delete_image(move |col, list_idx, id| {
        sn(d_r.borrow().clone());
        let mut d = d_r.borrow_mut();
        let start = start_date(*off_r.borrow());
        let task_opt: Option<&mut TaskRecord> = if col >= 0 {
            let date = start + Duration::days(col as i64);
            d.day_tasks.get_mut(&date_key(date))
                .and_then(|v| v.iter_mut().find(|t| t.id == id.as_str()))
        } else {
            d.someday_lists.get_mut(list_idx as usize)
                .and_then(|l| l.tasks.iter_mut().find(|t| t.id == id.as_str()))
        };
        if let Some(t) = task_opt { t.image_filename = None; }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }

    // ── file-drop-on-task: Bild per Drag & Drop auf Task ─────────────────────
    { let d_r = Rc::clone(&app_data); let off_r = Rc::clone(&week_offset);
      let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg);
      let sn = snapshot.clone(); let us_r = Rc::clone(&undo_stack); let rf = refresh.clone();
      ui.on_file_drop_on_task(move |col, list_idx, id, path_str| {
        let src = std::path::PathBuf::from(path_str.as_str());
        let Some(filename) = copy_image_to_store(&src, &fp_r.borrow()) else { return; };
        sn(d_r.borrow().clone());
        let mut d = d_r.borrow_mut();
        let start = start_date(*off_r.borrow());
        let task_opt: Option<&mut TaskRecord> = if col >= 0 {
            let date = start + Duration::days(col as i64);
            d.day_tasks.get_mut(&date_key(date))
                .and_then(|v| v.iter_mut().find(|t| t.id == id.as_str()))
        } else {
            d.someday_lists.get_mut(list_idx as usize)
                .and_then(|l| l.tasks.iter_mut().find(|t| t.id == id.as_str()))
        };
        if let Some(t) = task_opt { t.image_filename = Some(filename); }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }

    // ── set-task-color: Farbe einem Task zuweisen ────────────────────────────
    { let d_r = Rc::clone(&app_data); let off_r = Rc::clone(&week_offset);
      let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write); let cfg_r = Rc::clone(&cfg);
      let sn = snapshot.clone(); let us_r = Rc::clone(&undo_stack); let rf = refresh.clone();
      ui.on_set_task_color(move |col, list_idx, id, hex| {
        sn(d_r.borrow().clone());
        let mut d = d_r.borrow_mut();
        let color = if hex.is_empty() { None } else { Some(hex.to_string()) };
        let start = start_date(*off_r.borrow());
        let task_opt: Option<&mut TaskRecord> = if col >= 0 {
            let date = start + Duration::days(col as i64);
            d.day_tasks.get_mut(&date_key(date))
                .and_then(|v| v.iter_mut().find(|t| t.id == id.as_str()))
        } else {
            d.someday_lists.get_mut(list_idx as usize)
                .and_then(|l| l.tasks.iter_mut().find(|t| t.id == id.as_str()))
        };
        if let Some(t) = task_opt { t.color_hex = color; }
        save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r); drop(d); rf();
    }); }

    // quit-app button
    { let cfg_r = Rc::clone(&cfg);
      ui.on_quit_app(move || {
        save_config(&cfg_r.borrow());   // cfg mit aktueller mtime persistieren
        slint::quit_event_loop().ok();
    }); }

    // ════════════════════════════════════════════════════════════════════════
    //  System Tray  (Windows: native winapi impl via tray_win module)
    // ════════════════════════════════════════════════════════════════════════
    //
    //  Schlüssel: slint::run_event_loop_until_quit() statt ui.run().
    //  Damit bleibt der Event-Loop aktiv auch wenn kein Fenster sichtbar ist.
    //  HideWindow versteckt das Fenster; quit_event_loop() beendet die App.

    // X-Button → Fenster verstecken (Loop läuft weiter dank until_quit).
    ui.window().on_close_requested(|| slint::CloseRequestResponse::HideWindow);

    // Tray-Thread starten (no-op auf Nicht-Windows).
    let tray_rx = {
        let (rgba, w, h) = load_icon_rgba().unwrap_or_else(|| (vec![0u8; 4], 1, 1));
        tray_win::spawn(rgba, w, h)
    };

    // Tray-Events pollen → Fenster zeigen oder App beenden.
    let tray_timer = slint::Timer::default();
    {
        let ui_w = ui.as_weak();
        tray_timer.start(TimerMode::Repeated, std::time::Duration::from_millis(150), move || {
            while let Ok(ev) = tray_rx.try_recv() {
                match ev {
                    tray_win::TrayEvent::Show => {
                        if let Some(ui) = ui_w.upgrade() { ui.show().ok(); }
                    }
                    tray_win::TrayEvent::Quit => {
                        slint::quit_event_loop().ok();
                    }
                }
            }
        });
    }

    // ════════════════════════════════════════════════════════════════════════
    //  Notification timer (checks every 60 s on main thread)
    // ════════════════════════════════════════════════════════════════════════
    let notif_timer = slint::Timer::default();
    {
        let d_r  = Rc::clone(&app_data);
        let not_r = Rc::clone(&notified);
        notif_timer.start(TimerMode::Repeated, std::time::Duration::from_secs(60), move || {
            let d = d_r.borrow();
            check_notifications(&d, &mut not_r.borrow_mut());
        });
    }
    // Also check immediately at startup
    {
        let d = app_data.borrow();
        check_notifications(&d, &mut notified.borrow_mut());
    }

    // ════════════════════════════════════════════════════════════════════════
    //  Mitternacht-Timer: prüft sekündlich ob ein Tageswechsel stattgefunden hat
    //  → rollover_undone verschiebt offene Tasks auf heute
    // ════════════════════════════════════════════════════════════════════════
    let midnight_timer = slint::Timer::default();
    {
        let d_r   = Rc::clone(&app_data);
        let fp_r  = Rc::clone(&data_file);
        let low_r = Rc::clone(&last_own_write);
        let cfg_r = Rc::clone(&cfg);
        let us_r  = Rc::clone(&undo_stack);
        let rf    = refresh.clone();
        let mut last_date = Local::now().date_naive();
        midnight_timer.start(TimerMode::Repeated, std::time::Duration::from_secs(30), move || {
            let today = Local::now().date_naive();
            if today != last_date {
                last_date = today;
                let mut d = d_r.borrow_mut();
                rollover_undone(&mut d, today);
                save_and_record(&d, &fp_r.borrow(), &cfg_r, &us_r.borrow(), &low_r);
                drop(d);
                rf();
            }
        });
    }


    // ── Undo / Redo ──────────────────────────────────────────────────────────
    { let d_r = Rc::clone(&app_data); let us = Rc::clone(&undo_stack);
      let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write);
      let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      ui.on_undo(move || {
        let current = d_r.borrow().clone();
        let result  = us.borrow_mut().undo(current);   // mut-Borrow endet hier
        if let Some(prev) = result {
            *d_r.borrow_mut() = prev;                   // mut-Borrow endet hier
            {   // eigener Block: alle Borrows sicher gedroppt bevor rf() läuft
                let d  = d_r.borrow();
                let fp = fp_r.borrow();
                let us = us.borrow();
                save_and_record(&d, &fp, &cfg_r, &us, &low_r);
            }
            rf();
        }
    }); }
    { let d_r = Rc::clone(&app_data); let us = Rc::clone(&undo_stack);
      let fp_r = Rc::clone(&data_file); let low_r = Rc::clone(&last_own_write);
      let cfg_r = Rc::clone(&cfg); let rf = refresh.clone();
      ui.on_redo(move || {
        let current = d_r.borrow().clone();
        let result  = us.borrow_mut().redo(current);   // mut-Borrow endet hier
        if let Some(next) = result {
            *d_r.borrow_mut() = next;                   // mut-Borrow endet hier
            {
                let d  = d_r.borrow();
                let fp = fp_r.borrow();
                let us = us.borrow();
                save_and_record(&d, &fp, &cfg_r, &us, &low_r);
            }
            rf();
        }
    }); }
    // ════════════════════════════════════════════════════════════════════════
    //  File-Watcher-Timer: prüft alle 2s ob data.json extern geändert wurde.
    //  Bei externem Schreibvorgang: Daten neu laden, Undo-Stack leeren, UI refreshen.
    //  Eigene Schreibvorgänge werden via last_own_write ausgeblendet.
    //  Toleranz: +1s damit Cloud-Sync-Systeme (OneDrive, Dropbox) fertig schreiben können
    //  bevor wir lesen.
    // ════════════════════════════════════════════════════════════════════════
    let watch_timer = slint::Timer::default();
    {
        let d_r   = Rc::clone(&app_data);
        let fp_r  = Rc::clone(&data_file);
        let us_r  = Rc::clone(&undo_stack);
        let low_r = Rc::clone(&last_own_write);
        let cfg_r = Rc::clone(&cfg);
        let rf    = refresh.clone();
        watch_timer.start(TimerMode::Repeated, std::time::Duration::from_secs(2), move || {
            let fp = fp_r.borrow().clone();
            let Some(disk_mtime) = file_mtime_secs(&fp) else { return; };
            let own_mtime = *low_r.borrow();

            // Nur reagieren wenn die Datei neuer als unser letzter Schreibvorgang ist.
            // +1s Puffer: Cloud-Sync schreibt manchmal in mehreren Phasen.
            let is_extern = match own_mtime {
                Some(t) => disk_mtime > t + 1,
                None    => false,
            };
            if !is_extern { return; }

            // Neue Daten laden
            let new_data = load_data(&fp);

            // Rollover: vergangene Tasks auf heute verschieben (idempotent)
            let today = Local::now().date_naive();
            let mut patched = new_data.clone();
            rollover_undone(&mut patched, today);
            let needs_resave = serde_json::to_string(&patched).ok()
                != serde_json::to_string(&new_data).ok();

            *d_r.borrow_mut() = patched.clone();

            // Undo-Stack leeren: Snapshots vom anderen Gerät passen nicht zu unserem Stack
            *us_r.borrow_mut() = UndoStack::new();
            let _ = fs::remove_file(history_file(&fp));

            if needs_resave {
                // Rollover-Änderungen sofort zurückschreiben
                let d  = d_r.borrow();
                let us = us_r.borrow();
                save_and_record(&d, &fp, &cfg_r, &us, &low_r);
            } else {
                // Nur mtime aktualisieren damit wir nicht wieder triggern
                *low_r.borrow_mut() = Some(disk_mtime);
                cfg_r.borrow_mut().last_saved_secs = Some(disk_mtime);
            }

            rf();
        });
    }

    ui.show()?;
    // Timer am Leben halten bis zum Ende der Event-Loop
    let _t1 = tray_timer; let _t2 = notif_timer; let _t3 = midnight_timer; let _t4 = watch_timer;
    slint::run_event_loop_until_quit()
}

