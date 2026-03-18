# TuDu

A local task manager with a weekly view and a Someday section — built with **Rust** and **Slint**.

```
+--------+--------+--------+--------+--------+--------+---------------+
|  TUE   |  WED   |  THU   |  FRI   |  SAT   |  SUN   |  MON          |
| Mar 18 | Mar 19 | Mar 20 | Mar 21 | Mar 22 | Mar 23 | Mar 24        |
+--------+--------+--------+--------+--------+--------+---------------+
| [x]Task|        |        |        |        |        |               |
|    Task|  Task  |        |        |        |        |               |
| + Add  | + Add  | + Add  | + Add  | + Add  | + Add  | + Add task... |
+--------+--------+--------+--------+--------+--------+---------------+
| SOMEDAY                                             + New list      |
|  +-- SOMEDAY ----------+   +-- Ideas -----------+                   |
|  |  . Plan vacation    |   |  . Read a book     |                   |
|  |  + Add task...      |   |  + Add task...     |                   |
|  +---------------------+   +--------------------+                   |
+--------------------------------------------------------------------+
```

## Features

- **Weekly view** — 7 columns starting from today; navigate freely forward and backward
- **Tasks** — add (Enter), check off, rename (click), delete (hover → ×)
- **Due dates** — optionally with a time; overdue tasks are highlighted in color
- **Windows notifications** — reminder with date and time when a task becomes due
- **Drag & drop** — move tasks between day columns and Someday lists
- **Someday section** — any number of named lists for later; reorder lists via drag & drop
- **Image attachments** — attach an image to a task (📎 button or drag & drop an image file)
- **Task colors** — each task can have an individual color
- **Hide completed** — toggle completed tasks on/off; delete all at once
- **Undo / Redo** — unlimited undo/redo, persisted across restarts
- **Rollover** — incomplete tasks from past days are automatically moved to today
- **Multi-device sync** — external changes to `data.json` are detected; a banner appears
- **Settings** — accent color (8 presets) and data path configurable
- **System tray** — app keeps running in the background; reopen via tray icon
- **Single instance** — launching twice brings the existing window to the front
- **Local storage** — no cloud, no telemetry, no network

## Requirements

| Tool | Version |
| --- | --- |
| Rust + Cargo | ≥ 1.76 |
| C compiler (gcc / clang / MSVC) | required for Slint's native renderer |

On Linux, additionally required:

```bash
# Debian/Ubuntu
sudo apt install libxkbcommon-dev libfontconfig1-dev

# Fedora / RHEL
sudo dnf install libxkbcommon-devel fontconfig-devel
```

## Building & Running

```bash
# Release build (recommended)
cargo build --release
./target/release/tudu          # Linux/macOS
target\release\tudu.exe        # Windows

# Debug build (faster compilation)
cargo run
```

## Project Structure

```
tudu/
├── Cargo.toml              # dependencies
├── build.rs                # Slint compiler hook
├── assets/
│   ├── icon.ico / icon.png # app icon
│   └── icons/              # SVG icons used in the UI
├── ui/
│   └── appwindow.slint     # entire UI (Slint DSL)
└── src/
    ├── main.rs             # app logic, callbacks, persistence
    └── tray_win.rs         # Windows system tray integration
```

## Data Storage

Data is stored as human-readable JSON. The path can be changed in the Settings (⚙).

| OS | Default path |
| --- | --- |
| Windows | `%APPDATA%\tudu\data.json` |
| Linux | `~/.local/share/tudu/data.json` |
| macOS | `~/Library/Application Support/tudu/data.json` |

Also stored in the same folder as `data.json`:

- `data.history.json` — undo/redo history (persisted across restarts)
- `images/` — image attachments for tasks

The configuration (data path, accent color) is stored separately:

| OS | Config path |
| --- | --- |
| Windows | `%APPDATA%\tudu\config.json` |
| Linux | `~/.config/tudu/config.json` |
| macOS | `~/Library/Application Support/tudu/config.json` |

## Usage

| Action | How |
| --- | --- |
| Add a task | Type in the input field + **Enter** |
| Check off a task | Click the checkbox |
| Rename a task | Click the task text |
| Delete a task | Hover → click **×** |
| Move a task | Drag & drop to another column |
| Set a due date | Hover → click **📅** |
| Attach an image | Hover → click **📎**, or drag an image file onto the task |
| Set a task color | Hover → click the color dot |
| Navigate weeks | **‹** / **›** in the header, or **Today** |
| Toggle completed tasks | **👁** button in the header |
| Delete all completed | **🗑** button in the header |
| Undo / Redo | **↩** / **↪** in the header |
| Open settings | **⚙** button in the header |
| Add a Someday list | Click **+ New list** |
| Rename a Someday list | Click the list title |
| Reorder Someday lists | Drag the list header |
| Minimize to tray | Close the window — the app keeps running in the background |

## License

MIT — do what you like.

---

## Origin Story

This project was built entirely through **vibecoding** — the art of pestering an AI until a working program materializes. The author has not written, typed, or even knowingly looked at a single line of code.

All bugs were introduced by Claude (Anthropic), and all bugs were — after sufficiently firm requests — also fixed by Claude. The undo/redo icons, however, had to be rescued by Gemini (Google) after weeks of suffering, which goes to show: vibecoding is not a one-AI sport.

The result is a fully functional desktop application, proving that you can build productive software today without having the faintest idea what a borrow checker is.
