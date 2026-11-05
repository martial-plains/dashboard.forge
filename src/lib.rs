// SPDX-License-Identifier: GPL-3.0-or-later
//! Dashboard: an extensible startup screen, a port of emacs-dashboard
//! (https://github.com/emacs-dashboard/dashboard, GPL-3.0-or-later).
//!
//! `M-x dashboard-open` shows a banner, a title, an optional row of
//! navigator buttons, and lists of what matters most: recent files,
//! bookmarks, recent projects, the Org agenda and registers. RET (or a
//! click) opens the item under point; `r m p a e` jump to a section;
//! `n`/`p`/TAB move between items; `}` and `{` between sections; `1`-`9`
//! to the Nth section; DEL removes an item from its list; `g` refreshes.
//!
//! Settings, in init.rhai (all optional):
//!
//! ```rhai
//! forge.plugin.config("dashboard", #{
//!     startup: true,                  // open on *scratch* at startup
//!     title: "Welcome to Forge",      // or "" for none
//!     banner: "logo",                 // "logo" (the picture in a window, text in a
//!                                     // terminal), "forge", 1-4 (text banners),
//!                                     // "braille", "none", or a path to a text or
//!                                     // image file
//!     icons: true,                    // symbols by the headings and items (default:
//!                                     // on in a window, off in a terminal)
//!     center: true,                   // centre the content
//!     width: 80,                      // columns to centre in
//!     items: [["recents", 5], ["bookmarks", 5], ["projects", 5],
//!             ["agenda", 5], ["registers", 5]],
//!     shortcuts: true,                // show "(r)" after headings
//!     item_shortcuts: #{ recents: "r", bookmarks: "m", projects: "p",
//!                        agenda: "a", registers: "e" },
//!     item_names: #{ "Recent Files:": "Recently opened:" },
//!     navigation_cycle: false,        // wrap around at either end
//!     week_agenda: false,             // the agenda for 7 days, not today
//!     init_info: true,                // the "N files known" line
//!     footer: true,
//!     footer_messages: ["Happy hacking!"],
//!     navigator: [[["Homepage", "browse-url"], ["Scratch", "scratch-buffer"]]],
//! });
//! ```
//!
//!   cargo build --release --target wasm32-wasip2
//!   cp target/wasm32-wasip2/release/forge_dashboard.wasm \
//!      ~/.config/forge/plugins/dashboard.wasm

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use forge_plugin_sdk::{
    Buffer, Position, Span, bind_in_mode, command, config_json, environment, files,
    forge_plugin, images, message, modes, org, registers, run_command, session,
};
use serde_json::Value;

const MODE: &str = "dashboard-mode";
const BUFFER: &str = "*dashboard*";

const BANNERS: [&str; 4] = [
    include_str!("../banners/1.txt"),
    include_str!("../banners/2.txt"),
    include_str!("../banners/3.txt"),
    include_str!("../banners/4.txt"),
];
const BRAILLE: &str = include_str!("../banners/logo-braille.txt");
const FORGE: &str = include_str!("../banners/forge.txt");

// ---------------------------------------------------------- settings

#[derive(Debug, Clone, PartialEq)]
pub enum Banner {
    None,
    Forge,
    /// The Forge logo as a picture where pictures are drawn, otherwise
    /// the text banner.
    Logo,
    Numbered(usize),
    Braille,
    /// A text or image file.
    Path(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub startup: bool,
    pub title: String,
    pub banner: Banner,
    /// Symbols by headings and items; None: when the frontend is a window.
    pub icons: Option<bool>,
    pub center: bool,
    pub width: usize,
    /// Centre in the window's own width (until `width` is set).
    pub width_auto: bool,
    pub items: Vec<(String, usize)>,
    pub shortcuts: bool,
    pub item_shortcuts: BTreeMap<String, String>,
    pub item_names: BTreeMap<String, String>,
    pub cycle: bool,
    pub week_agenda: bool,
    pub init_info: bool,
    pub footer: bool,
    pub footer_messages: Vec<String>,
    /// Rows of (label, command) buttons.
    pub navigator: Vec<Vec<(String, String)>>,
    pub heading_shortcut_format: String,
}

impl Default for Config {
    fn default() -> Self {
        let sc = |k: &str, v: &str| (k.to_string(), v.to_string());
        Self {
            startup: true,
            title: "Welcome to Forge".into(),
            banner: Banner::Logo,
            icons: None,
            center: true,
            width: 80,
            width_auto: true,
            items: ["recents", "bookmarks", "projects", "agenda", "registers"]
                .iter()
                .map(|s| (s.to_string(), 5))
                .collect(),
            shortcuts: true,
            item_shortcuts: [
                sc("recents", "r"),
                sc("bookmarks", "m"),
                sc("projects", "p"),
                sc("agenda", "a"),
                sc("registers", "e"),
            ]
            .into_iter()
            .collect(),
            item_names: BTreeMap::new(),
            cycle: false,
            week_agenda: false,
            init_info: true,
            footer: true,
            footer_messages: vec![
                "Happy hacking!".into(),
                "Forge it your way.".into(),
                "Everything is a buffer.".into(),
                "Welcome back.".into(),
            ],
            navigator: Vec::new(),
            heading_shortcut_format: " ({})".into(),
        }
    }
}

const KNOWN: [&str; 17] = [
    "startup",
    "title",
    "banner",
    "icons",
    "center",
    "width",
    "items",
    "shortcuts",
    "item_shortcuts",
    "item_names",
    "navigation_cycle",
    "week_agenda",
    "init_info",
    "footer",
    "footer_messages",
    "navigator",
    "heading_shortcut_format",
];

pub const SECTIONS: [&str; 5] = ["recents", "bookmarks", "projects", "agenda", "registers"];

pub fn parse_config(json: &str) -> Result<Config, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| format!("dashboard: settings: {e}"))?;
    let Some(map) = value.as_object() else {
        return Err("dashboard: settings must be a table".into());
    };
    let mut c = Config::default();
    let bool_of = |k: &str, v: &Value| {
        v.as_bool()
            .ok_or_else(|| format!("dashboard: {k} must be true or false"))
    };
    let str_of = |k: &str, v: &Value| {
        v.as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("dashboard: {k} must be a string"))
    };
    for (key, v) in map {
        match key.as_str() {
            "startup" => c.startup = bool_of(key, v)?,
            "center" => c.center = bool_of(key, v)?,
            "icons" => c.icons = Some(bool_of(key, v)?),
            "shortcuts" => c.shortcuts = bool_of(key, v)?,
            "navigation_cycle" => c.cycle = bool_of(key, v)?,
            "week_agenda" => c.week_agenda = bool_of(key, v)?,
            "init_info" => c.init_info = bool_of(key, v)?,
            "footer" => c.footer = bool_of(key, v)?,
            "title" => c.title = str_of(key, v)?,
            "heading_shortcut_format" => c.heading_shortcut_format = str_of(key, v)?,
            "width" => {
                c.width = v
                    .as_u64()
                    .filter(|w| (20..=400).contains(w))
                    .ok_or("dashboard: width must be a number from 20 to 400")?
                    as usize;
                c.width_auto = false;
            }
            "banner" => {
                c.banner = match v {
                    Value::Number(n) => {
                        let i = n.as_u64().unwrap_or(0) as usize;
                        if (1..=BANNERS.len()).contains(&i) {
                            Banner::Numbered(i)
                        } else {
                            return Err("dashboard: banner number must be 1 to 4".into());
                        }
                    }
                    Value::String(s) => match s.as_str() {
                        "none" | "" => Banner::None,
                        "forge" => Banner::Forge,
                        "logo" | "official" | "image" | "auto" => Banner::Logo,
                        "braille" | "logo-braille" => Banner::Braille,
                        path => Banner::Path(path.to_string()),
                    },
                    _ => return Err("dashboard: banner must be a name, 1-4 or a path".into()),
                };
            }
            "items" => {
                let list = v.as_array().ok_or("dashboard: items must be a list")?;
                let mut items = Vec::new();
                for it in list {
                    let (name, n) = match it {
                        Value::String(s) => (s.clone(), 20),
                        Value::Array(a) if !a.is_empty() => (
                            a[0].as_str().ok_or("dashboard: an item starts with its name")?.to_string(),
                            a.get(1).and_then(Value::as_u64).unwrap_or(20) as usize,
                        ),
                        _ => return Err("dashboard: an item is [\"name\", count]".into()),
                    };
                    if !SECTIONS.contains(&name.as_str()) {
                        return Err(format!(
                            "dashboard: unknown item {name:?} (known: {})",
                            SECTIONS.join(", ")
                        ));
                    }
                    items.push((name, n));
                }
                c.items = items;
            }
            "item_shortcuts" | "item_names" => {
                let m = v.as_object().ok_or_else(|| format!("dashboard: {key} must be a table"))?;
                let mut out = BTreeMap::new();
                for (k, v) in m {
                    out.insert(k.clone(), str_of(key, v)?);
                }
                if key == "item_shortcuts" {
                    c.item_shortcuts = out;
                } else {
                    c.item_names = out;
                }
            }
            "footer_messages" => {
                let a = v.as_array().ok_or("dashboard: footer_messages must be a list")?;
                c.footer_messages = a
                    .iter()
                    .map(|s| s.as_str().map(str::to_string))
                    .collect::<Option<_>>()
                    .ok_or("dashboard: footer_messages are strings")?;
            }
            "navigator" => {
                let rows = v.as_array().ok_or("dashboard: navigator is a list of rows")?;
                let mut out = Vec::new();
                for row in rows {
                    let btns = row.as_array().ok_or("dashboard: a navigator row is a list")?;
                    let mut r = Vec::new();
                    for b in btns {
                        let a = b.as_array().filter(|a| a.len() == 2);
                        let a = a.ok_or("dashboard: a navigator button is [\"label\", \"command\"]")?;
                        let (Some(l), Some(cmd)) = (a[0].as_str(), a[1].as_str()) else {
                            return Err("dashboard: a navigator button is two strings".into());
                        };
                        r.push((l.to_string(), cmd.to_string()));
                    }
                    out.push(r);
                }
                c.navigator = out;
            }
            other => {
                return Err(format!(
                    "dashboard: unknown setting {other:?} (known: {})",
                    KNOWN.join(", ")
                ));
            }
        }
    }
    Ok(c)
}

// ------------------------------------------------------------- model

/// What RET does on an item.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    File(String),
    Bookmark { path: String, offset: u32 },
    Project(String),
    Agenda { buffer: u32, line: u32 },
    Register(String),
    Command(String),
}

/// What the lists are built from.
#[derive(Debug, Clone, Default)]
pub struct Data {
    pub recents: Vec<String>,
    /// (name, path, offset)
    pub bookmarks: Vec<(String, String, u32)>,
    pub projects: Vec<String>,
    /// (display text, buffer, line)
    pub agenda: Vec<(String, u32, u32)>,
    /// (name, description)
    pub registers: Vec<(String, String)>,
    pub files_known: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub line: usize,
    pub col: usize,
    pub section: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deco {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub face: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct Page {
    pub text: String,
    pub decos: Vec<Deco>,
    pub items: Vec<Item>,
    /// (section, first item line, shortcut)
    pub sections: Vec<(String, usize, Option<String>)>,
    /// A picture banner goes under this line.
    pub image: Option<(usize, String)>,
}

fn pad(line: &str, width: usize, block_w: usize, center: bool) -> (String, usize) {
    if !center {
        return (line.to_string(), 0);
    }
    let _ = line;
    let left = width.saturating_sub(block_w) / 2;
    (format!("{}{line}", " ".repeat(left)), left)
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn default_heading(section: &str, week: bool) -> &'static str {
    match section {
        "recents" => "Recent Files:",
        "bookmarks" => "Bookmarks:",
        "projects" => "Projects:",
        "agenda" if week => "Agenda for the coming week:",
        "agenda" => "Agenda for today:",
        _ => "Registers:",
    }
}

/// `/home/me/src/x.rs` -> `~/src/x.rs`.
pub fn abbreviate(path: &str, home: &str) -> String {
    if !home.is_empty()
        && let Some(rest) = path.strip_prefix(home)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("~{rest}");
    }
    path.to_string()
}

fn base_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

/// Shorten the middle of an over-long path.
pub fn shorten(s: &str, max: usize) -> String {
    let n = chars(s);
    if n <= max || max < 8 {
        return s.to_string();
    }
    let keep = max - 1;
    let head = keep / 3;
    let tail = keep - head;
    let a: String = s.chars().take(head).collect();
    let b: String = s.chars().skip(n - tail).collect();
    format!("{a}…{b}")
}

struct Builder {
    lines: Vec<String>,
    decos: Vec<Deco>,
    items: Vec<Item>,
}

impl Builder {
    fn blank(&mut self) {
        self.lines.push(String::new());
    }

    /// Add `texts` as a block centred together; returns the first line.
    fn block(
        &mut self,
        texts: &[String],
        cfg: &Config,
        face: Option<&'static str>,
        each: bool,
    ) -> usize {
        let w = texts.iter().map(|t| chars(t)).max().unwrap_or(0);
        let first = self.lines.len();
        for t in texts {
            let bw = if each { chars(t) } else { w };
            let (line, left) = pad(t, cfg.width, bw, cfg.center);
            if let Some(f) = face
                && !t.is_empty()
            {
                self.decos.push(Deco {
                    line: self.lines.len(),
                    start: left,
                    end: left + chars(t),
                    face: f,
                });
            }
            self.lines.push(line);
        }
        first
    }
}

/// Build the page: a pure function of the settings and the data.
pub fn build(cfg: &Config, data: &Data, home: &str, today: &str, seed: u64) -> Page {
    let mut b = Builder {
        lines: Vec::new(),
        decos: Vec::new(),
        items: Vec::new(),
    };
    let mut sections = Vec::new();
    let mut image = None;
    b.blank();
    // Banner.
    match &cfg.banner {
        Banner::None => {}
        Banner::Path(p) if is_image_path(p) => {
            // Blank rows under this line are reserved for the picture.
            let at = b.lines.len();
            b.block(&[String::new()], cfg, None, false);
            image = Some((at, p.clone()));
        }
        banner => {
            let text = match banner {
                Banner::Forge | Banner::Logo => FORGE.to_string(),
                Banner::Numbered(i) => BANNERS[i - 1].to_string(),
                Banner::Braille => BRAILLE.to_string(),
                Banner::Path(p) => files::read(p).unwrap_or_default(),
                Banner::None => String::new(),
            };
            let lines: Vec<String> = text.lines().map(str::to_string).collect();
            if !lines.is_empty() {
                b.block(&lines, cfg, Some("constant"), false);
            }
        }
    }
    b.blank();
    if !cfg.title.is_empty() {
        b.block(&[cfg.title.clone()], cfg, Some("type"), false);
        b.blank();
    }
    // Navigator buttons.
    for row in &cfg.navigator {
        let text = row
            .iter()
            .map(|(l, _)| format!("[{l}]"))
            .collect::<Vec<_>>()
            .join(" ");
        let w = chars(&text);
        let (line, left) = pad(&text, cfg.width, w, cfg.center);
        let ln = b.lines.len();
        let mut col = left;
        for (l, command) in row {
            let len = chars(l) + 2;
            b.decos.push(Deco {
                line: ln,
                start: col,
                end: col + len,
                face: "link",
            });
            b.items.push(Item {
                line: ln,
                col,
                section: "navigator".into(),
                action: Action::Command(command.clone()),
            });
            col += len + 1;
        }
        b.lines.push(line);
    }
    if !cfg.navigator.is_empty() {
        b.blank();
    }
    if cfg.init_info {
        b.block(
            &[format!(
                "{} recent files known · {today}",
                data.files_known
            )],
            cfg,
            Some("comment"),
            false,
        );
    }
    // The lists sit at the window's left edge, as in emacs-dashboard; only
    // the banner, title, buttons and footer are centred.
    let list_left = 0usize;
    // The lists.
    for (name, count) in &cfg.items {
        b.blank();
        let heading_default = default_heading(name, cfg.week_agenda);
        let heading = cfg
            .item_names
            .get(heading_default)
            .map_or(heading_default, String::as_str)
            .to_string();
        let shortcut = cfg
            .item_shortcuts
            .get(name)
            .filter(|s| !s.is_empty())
            .cloned();
        let rows = rows_for(name, data, home, *count, cfg);
        let shown = shortcut
            .as_ref()
            .filter(|_| cfg.shortcuts && !rows.is_empty())
            .map(|s| cfg.heading_shortcut_format.replace("{}", s))
            .unwrap_or_default();
        let icons = cfg.icons.unwrap_or(false);
        let heading = if icons { format!("{} {heading}", section_icon(name)) } else { heading };
        let full_heading = format!("{heading}{shown}");
        let mut block = vec![full_heading.clone()];
        if rows.is_empty() {
            block.push("    --- No items ---".to_string());
        } else if icons {
            block.extend(rows.iter().map(|(t, _)| format!("  \u{25AB} {t}")));
        } else {
            block.extend(rows.iter().map(|(t, _)| format!("    {t}")));
        }
        let first = b.lines.len();
        let left = list_left;
        for (i, t) in block.iter().enumerate() {
            let ln = first + i;
            b.lines.push(format!("{}{t}", " ".repeat(left)));
            if i == 0 {
                let hl = chars(&heading);
                b.decos.push(Deco { line: ln, start: left, end: left + hl, face: "function" });
                if !shown.is_empty() {
                    b.decos.push(Deco {
                        line: ln,
                        start: left + hl,
                        end: left + chars(&full_heading),
                        face: "comment",
                    });
                }
            } else if rows.is_empty() {
                b.decos.push(Deco {
                    line: ln,
                    start: left,
                    end: left + chars(t),
                    face: "comment",
                });
            } else {
                let (text, action) = &rows[i - 1];
                // With icons the path after the name is dimmed, as in
                // emacs-dashboard's window.
                let split = if icons { text.find("  ").map(|at| chars(&text[..at])) } else { None };
                b.decos.push(Deco {
                    line: ln,
                    start: left + 4,
                    end: left + 4 + split.unwrap_or_else(|| chars(text)),
                    face: "link",
                });
                if let Some(at) = split {
                    b.decos.push(Deco {
                        line: ln,
                        start: left + 4 + at,
                        end: left + 4 + chars(text),
                        face: "comment",
                    });
                }
                if icons {
                    b.decos.push(Deco { line: ln, start: left + 2, end: left + 3, face: "keyword" });
                }
                b.items.push(Item {
                    line: ln,
                    col: left + 4,
                    section: name.clone(),
                    action: action.clone(),
                });
            }
        }
        let first_item = if rows.is_empty() { None } else { Some(first + 1) };
        if let Some(l) = first_item {
            sections.push((name.clone(), l, shortcut));
        }
    }
    b.blank();
    if cfg.footer && !cfg.footer_messages.is_empty() {
        let msg = &cfg.footer_messages[(seed as usize) % cfg.footer_messages.len()];
        b.block(&[msg.clone()], cfg, Some("comment"), false);
        b.blank();
    }
    Page {
        text: b.lines.join("\n") + "\n",
        decos: b.decos,
        items: b.items,
        sections,
        image,
    }
}

fn is_image_path(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"]
        .iter()
        .any(|e| l.ends_with(e))
}

/// The symbol before a section's heading (emacs-dashboard shows an icon
/// there); plain one-cell symbols, so they need no icon font.
fn section_icon(section: &str) -> char {
    match section {
        "recents" => '\u{25D4}',
        "bookmarks" => '\u{2605}',
        "projects" => '\u{25A3}',
        "agenda" => '\u{25C8}',
        _ => '\u{203B}',
    }
}

/// The settings with what depends on the frontend settled: the logo
/// picture in a window (the text banner in a terminal), and icons.
fn resolve(mut cfg: Config) -> Config {
    let graphic = forge_plugin_sdk::environment::display_graphic();
    if cfg.banner == Banner::Logo {
        cfg.banner = match graphic.then(|| images::builtin("logo")).flatten() {
            Some(path) => Banner::Path(path),
            None => Banner::Forge,
        };
    }
    cfg.icons = Some(cfg.icons.unwrap_or(graphic));
    if cfg.width_auto {
        let w = forge_plugin_sdk::environment::frame_width() as usize;
        if w > 0 {
            cfg.width = w.clamp(40, 400);
        }
    }
    cfg
}

/// The (text, action) rows of one section, at most `count`.
fn rows_for(
    name: &str,
    data: &Data,
    home: &str,
    count: usize,
    cfg: &Config,
) -> Vec<(String, Action)> {
    let _ = cfg;
    match name {
        "recents" => data
            .recents
            .iter()
            .take(count)
            .map(|p| {
                (
                    format!("{}  {}", base_name(p), shorten(&abbreviate(p, home), 60)),
                    Action::File(p.clone()),
                )
            })
            .collect(),
        "bookmarks" => data
            .bookmarks
            .iter()
            .take(count)
            .map(|(n, p, off)| {
                (
                    format!("{n} - {}", shorten(&abbreviate(p, home), 60)),
                    Action::Bookmark { path: p.clone(), offset: *off },
                )
            })
            .collect(),
        "projects" => data
            .projects
            .iter()
            .take(count)
            .map(|p| {
                (
                    format!("{}  {}", base_name(p), shorten(&abbreviate(p, home), 60)),
                    Action::Project(p.clone()),
                )
            })
            .collect(),
        "agenda" => data
            .agenda
            .iter()
            .take(count)
            .map(|(t, b, l)| (t.clone(), Action::Agenda { buffer: *b, line: *l }))
            .collect(),
        _ => data
            .registers
            .iter()
            .take(count)
            .map(|(n, d)| (format!("{n}: {d}"), Action::Register(n.clone())))
            .collect(),
    }
}

/// The recent projects: the project root of each recent file, in the
/// order they were last used, once each.
pub fn projects_of(recents: &[String], root_of: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for f in recents {
        if let Some(r) = root_of(f)
            && seen.insert(r.clone())
        {
            out.push(r);
        }
    }
    out
}

// -------------------------------------------------------- navigation

/// The item after (or before) the cursor, optionally wrapping.
pub fn step(items: &[Item], line: usize, col: usize, forward: bool, cycle: bool) -> Option<usize> {
    if items.is_empty() {
        return None;
    }
    let here = (line, col);
    let pos = |i: &Item| (i.line, i.col);
    let idx = if forward {
        items.iter().position(|i| pos(i) > here)
    } else {
        items.iter().rposition(|i| pos(i) < here)
    };
    match idx {
        Some(i) => Some(i),
        None if cycle => Some(if forward { 0 } else { items.len() - 1 }),
        None => None,
    }
}

/// The item the cursor is on or, failing that, on the same line.
pub fn item_at(items: &[Item], line: usize) -> Option<usize> {
    items.iter().position(|i| i.line == line)
}

/// The first item of the section after (before) the cursor's line.
pub fn section_step(
    sections: &[(String, usize, Option<String>)],
    line: usize,
    forward: bool,
    cycle: bool,
) -> Option<usize> {
    if sections.is_empty() {
        return None;
    }
    let idx = if forward {
        sections.iter().position(|s| s.1 > line)
    } else {
        sections.iter().rposition(|s| s.1 < line)
    };
    match idx {
        Some(i) => Some(sections[i].1),
        None if cycle => Some(if forward { sections[0].1 } else { sections[sections.len() - 1].1 }),
        None => None,
    }
}

// ---------------------------------------------------- the live state

thread_local! {
    static CONFIG: RefCell<Config> = RefCell::new(Config::default());
    static PAGE: RefCell<Page> = RefCell::new(Page::default());
    /// Items the user removed with DEL (until the next restart).
    static REMOVED: RefCell<BTreeSet<(String, String)>> = const { RefCell::new(BTreeSet::new()) };
}

fn key_of(a: &Action) -> (String, String) {
    match a {
        Action::File(p) | Action::Project(p) => ("path".into(), p.clone()),
        Action::Bookmark { path, .. } => ("bookmark".into(), path.clone()),
        Action::Agenda { buffer, line } => ("agenda".into(), format!("{buffer}:{line}")),
        Action::Register(n) => ("register".into(), n.clone()),
        Action::Command(c) => ("command".into(), c.clone()),
    }
}

fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    // Civil date from days since 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn gather(cfg: &Config) -> Data {
    let removed = REMOVED.with(|r| r.borrow().clone());
    let keep = |a: Action| !removed.contains(&key_of(&a));
    let recents_all = session::recent_files();
    let recents: Vec<String> = recents_all
        .iter()
        .filter(|p| keep(Action::File((*p).clone())))
        .cloned()
        .collect();
    let projects: Vec<String> = projects_of(&recents_all, |f| forge_plugin_sdk::project_root(f))
        .into_iter()
        .filter(|p| keep(Action::Project(p.clone())))
        .collect();
    let bookmarks = session::bookmark_names()
        .into_iter()
        .filter_map(|n| session::bookmark(&n))
        .filter(|b| keep(Action::Bookmark { path: b.path.clone(), offset: b.offset }))
        .map(|b| (b.name, b.path, b.offset))
        .collect();
    let wants = |s: &str| cfg.items.iter().any(|(n, _)| n == s);
    let agenda = if wants("agenda") {
        let days = if cfg.week_agenda { 7 } else { 1 };
        org::span(days, None, false)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.done && keep(Action::Agenda { buffer: e.buffer, line: e.line }))
            .map(|e| {
                let kw = e.keyword.map(|k| format!("{k} ")).unwrap_or_default();
                let when = e.time.or(e.date).map(|t| format!("{t}  ")).unwrap_or_default();
                (
                    format!("{}  {when}{kw}{}", e.category, e.title),
                    e.buffer,
                    e.line,
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let regs = registers::names()
        .into_iter()
        .filter(|n| keep(Action::Register(n.clone())))
        .map(|n| {
            let d = registers::describe(&n).unwrap_or_default();
            (n, d)
        })
        .collect();
    Data {
        recents: recents.clone(),
        bookmarks,
        projects,
        agenda,
        registers: regs,
        files_known: recents_all.len(),
    }
}

fn home() -> String {
    environment::get("HOME").ok().flatten().unwrap_or_default()
}

fn whole() -> Span {
    Span {
        start: Position { line: 0, column: 0 },
        end: Position { line: u32::MAX, column: u32::MAX },
    }
}

/// Build and show the dashboard, keeping point where it was when asked.
fn render(show: bool) -> Result<(), String> {
    let cfg = resolve(CONFIG.with(|c| c.borrow().clone()));
    let data = gather(&cfg);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 3600)
        .unwrap_or(0);
    let page = build(&cfg, &data, &home(), &today(), seed);
    let buffer = Buffer::find(BUFFER).unwrap_or_else(|| Buffer::create(BUFFER));
    let before = buffer.point();
    buffer.set_major_mode(MODE)?;
    buffer.set_option("read-only", "false")?;
    buffer.replace(whole(), &page.text);
    buffer.set_option("read-only", "true")?;
    let decos: Vec<_> = page
        .decos
        .iter()
        .map(|d| forge_plugin_sdk::Decoration {
            span: Span {
                start: Position { line: d.line as u32, column: d.start as u32 },
                end: Position { line: d.line as u32, column: d.end as u32 },
            },
            face: d.face.to_string(),
            message: None,
        })
        .collect();
    let _ = forge_plugin_sdk::decorations::set(&buffer, &decos);
    match &page.image {
        Some((line, path)) => images::set_centered(
            &buffer,
            &[forge_plugin_sdk::Image { line: *line as u32, path: path.clone() }],
        ),
        None => images::clear(&buffer),
    }
    let first = page.items.first().map(|i| (i.line, i.col));
    PAGE.with(|p| *p.borrow_mut() = page);
    if show {
        buffer.show();
        if let Some((l, c)) = first {
            buffer.set_point(Position { line: l as u32, column: c as u32 });
        }
    } else {
        buffer.set_point(before);
    }
    Ok(())
}

fn here() -> (usize, usize) {
    let p = Buffer::current().point();
    (p.line as usize, p.column as usize)
}

fn goto(line: usize, col: usize) {
    Buffer::current().set_point(Position { line: line as u32, column: col as u32 });
}

fn in_dashboard() -> Result<(), String> {
    if Buffer::current().name() == BUFFER {
        Ok(())
    } else {
        Err("Not in the dashboard".into())
    }
}

fn move_item(forward: bool) -> Result<(), String> {
    in_dashboard()?;
    let (l, c) = here();
    let cycle = CONFIG.with(|c| c.borrow().cycle);
    PAGE.with(|p| {
        let p = p.borrow();
        if let Some(i) = step(&p.items, l, c, forward, cycle) {
            goto(p.items[i].line, p.items[i].col);
        }
    });
    Ok(())
}

fn move_section(forward: bool) -> Result<(), String> {
    in_dashboard()?;
    let (l, _) = here();
    let cycle = CONFIG.with(|c| c.borrow().cycle);
    PAGE.with(|p| {
        let p = p.borrow();
        if let Some(line) = section_step(&p.sections, l, forward, cycle)
            && let Some(i) = item_at(&p.items, line)
        {
            goto(p.items[i].line, p.items[i].col);
        }
    });
    Ok(())
}

fn goto_nth_section(n: usize) -> Result<(), String> {
    in_dashboard()?;
    PAGE.with(|p| {
        let p = p.borrow();
        if let Some((_, line, _)) = p.sections.get(n.saturating_sub(1))
            && let Some(i) = item_at(&p.items, *line)
        {
            goto(p.items[i].line, p.items[i].col);
        } else {
            message("No such section");
        }
    });
    Ok(())
}

fn goto_shortcut(key: &str) -> Result<(), String> {
    in_dashboard()?;
    PAGE.with(|p| {
        let p = p.borrow();
        if let Some((_, line, _)) = p.sections.iter().find(|s| s.2.as_deref() == Some(key))
            && let Some(i) = item_at(&p.items, *line)
        {
            goto(p.items[i].line, p.items[i].col);
        }
    });
    Ok(())
}

fn current_action() -> Option<Action> {
    let (l, _) = here();
    PAGE.with(|p| {
        let p = p.borrow();
        item_at(&p.items, l).map(|i| p.items[i].action.clone())
    })
}

fn open_item() -> Result<(), String> {
    in_dashboard()?;
    let Some(action) = current_action() else {
        return Ok(());
    };
    match action {
        // What the user pressed Enter on opens with no directory grant: the
        // choice is theirs, not the plugin's.
        Action::File(p) | Action::Project(p) => {
            if let Some(b) = Buffer::open_path(&p)? {
                b.show();
            }
        }
        Action::Bookmark { path, offset } => {
            if let Some(b) = Buffer::open_path(&path)? {
                b.show();
                b.set_point(offset_to_position(&b.text(), offset));
            }
        }
        Action::Agenda { buffer, line } => {
            if let Some(b) = Buffer::from_id(buffer).filter(Buffer::alive) {
                b.show();
                b.set_point(Position { line, column: 0 });
            }
        }
        Action::Register(name) => {
            if let Some(m) = registers::position(&name) {
                if let Some(b) = Buffer::from_id(m.buffer).filter(Buffer::alive) {
                    b.show();
                    b.set_point(offset_to_position(&b.text(), m.offset));
                }
            } else if let Some(t) = registers::text(&name) {
                message(&format!("Register {name}: {t}"));
            }
        }
        Action::Command(c) => run_command(&c)?,
    }
    Ok(())
}

fn offset_to_position(text: &str, offset: u32) -> Position {
    let mut line = 0;
    let mut col = 0;
    for (i, ch) in text.chars().enumerate() {
        if i as u32 >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position { line, column: col }
}

fn remove_item() -> Result<(), String> {
    in_dashboard()?;
    let Some(action) = current_action() else {
        return Ok(());
    };
    if matches!(action, Action::Command(_)) {
        return Ok(());
    }
    REMOVED.with(|r| r.borrow_mut().insert(key_of(&action)));
    let (l, c) = here();
    render(false)?;
    goto(l, c);
    message("Removed from the dashboard");
    Ok(())
}

fn open() -> Result<(), String> {
    render(true)
}

fn init() -> Result<(), String> {
    let cfg = parse_config(&config_json())?;
    CONFIG.with(|c| *c.borrow_mut() = cfg.clone());
    modes::define_major_mode(MODE, Some("special-mode"), &[])?;
    command("dashboard-open", "Show the dashboard.", |_| open())?;
    command(
        "dashboard-refresh-buffer",
        "Rebuild and show the dashboard (g).",
        |_| open(),
    )?;
    command("dashboard-return", "Open the item under point (RET).", |_| open_item())?;
    command("dashboard-next-item", "Move to the next item (n, TAB).", |_| move_item(true))?;
    command("dashboard-previous-item", "Move to the previous item (p, S-TAB).", |_| {
        move_item(false)
    })?;
    command("dashboard-next-section", "Move to the next section (}).", |_| {
        move_section(true)
    })?;
    command("dashboard-previous-section", "Move to the previous section ({).", |_| {
        move_section(false)
    })?;
    command(
        "dashboard-remove-item-under",
        "Remove the item under point from its list (DEL).",
        |_| remove_item(),
    )?;
    for n in 1..=9usize {
        let name = format!("dashboard-section-{n}");
        command(&name, &format!("Go to section {n}."), move |_| goto_nth_section(n))?;
        bind_in_mode(MODE, &n.to_string(), &name)?;
    }
    for (keys, cmd) in [
        ("RET", "dashboard-return"),
        ("n", "dashboard-next-item"),
        ("C-n", "dashboard-next-item"),
        ("j", "dashboard-next-item"),
        ("<down>", "dashboard-next-item"),
        ("TAB", "dashboard-next-item"),
        ("C-p", "dashboard-previous-item"),
        ("k", "dashboard-previous-item"),
        ("<up>", "dashboard-previous-item"),
        ("<backtab>", "dashboard-previous-item"),
        ("}", "dashboard-next-section"),
        ("{", "dashboard-previous-section"),
        ("DEL", "dashboard-remove-item-under"),
        ("<deletechar>", "dashboard-remove-item-under"),
        ("g", "dashboard-refresh-buffer"),
    ] {
        bind_in_mode(MODE, keys, cmd)?;
    }
    // `p` is projects' shortcut by default, as in the original; C-p and k
    // move up. A click opens the item under it.
    let _ = bind_in_mode(MODE, "<mouse-1>", "dashboard-return");
    // One command per section shortcut, bound to its key.
    for (section, key) in cfg.item_shortcuts.iter() {
        if key.is_empty() {
            continue;
        }
        let cmd = format!("dashboard-jump-to-{section}");
        let k = key.clone();
        command(&cmd, &format!("Go to the {section} section."), move |_| goto_shortcut(&k))?;
        bind_in_mode(MODE, key, &cmd)?;
    }
    if cfg.startup && Buffer::current().name() == "*scratch*" {
        render(true)?;
    }
    Ok(())
}

forge_plugin!(init);

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> Data {
        Data {
            recents: vec!["/home/me/a.rs".into(), "/home/me/src/b.rs".into()],
            bookmarks: vec![("notes".into(), "/home/me/notes.org".into(), 12)],
            projects: vec!["/home/me/src".into()],
            agenda: vec![("work  09:00  TODO ship".into(), 3, 7)],
            registers: vec![("a".into(), "text: hi".into())],
            files_known: 2,
        }
    }

    fn page() -> Page {
        build(&Config::default(), &data(), "/home/me", "2026-11-02", 0)
    }

    #[test]
    fn builds_every_section_with_a_shortcut_and_items() {
        let p = page();
        assert!(p.text.contains("Recent Files: (r)"), "{}", p.text);
        assert!(p.text.contains("Bookmarks: (m)"));
        assert!(p.text.contains("Projects: (p)"));
        assert!(p.text.contains("Agenda for today: (a)"));
        assert!(p.text.contains("Registers: (e)"));
        assert!(p.text.contains("a.rs  ~/a.rs"));
        assert_eq!(p.items.len(), 6);
        assert_eq!(p.sections.len(), 5);
        // Each item's line really holds its text at its column.
        let lines: Vec<&str> = p.text.lines().collect();
        for i in &p.items {
            let l = lines[i.line];
            assert!(l.chars().nth(i.col).is_some_and(|c| c != ' '), "{l:?} at {}", i.col);
        }
    }

    #[test]
    fn empty_lists_say_so_and_have_no_shortcut() {
        let p = build(&Config::default(), &Data::default(), "", "d", 0);
        assert!(p.text.contains("--- No items ---"));
        assert!(!p.text.contains("(r)"));
        assert!(p.items.is_empty());
    }

    #[test]
    fn counts_limit_each_list_and_names_can_change() {
        let mut c = Config::default();
        c.items = vec![("recents".into(), 1)];
        c.item_names.insert("Recent Files:".into(), "Lately:".into());
        c.center = false;
        let p = build(&c, &data(), "/home/me", "d", 0);
        assert!(p.text.contains("Lately: (r)"));
        assert_eq!(p.items.len(), 1);
    }

    #[test]
    fn content_is_centred_in_the_width() {
        let mut c = Config::default();
        c.items = vec![];
        c.banner = Banner::None;
        c.title = "HI".into();
        c.init_info = false;
        c.footer = false;
        let p = build(&c, &Data::default(), "", "d", 0);
        let l = p.text.lines().find(|l| l.contains("HI")).unwrap();
        assert_eq!(l.find("HI"), Some(39));
    }

    #[test]
    fn navigation_steps_between_items_and_sections() {
        let p = page();
        let first = &p.items[0];
        let i = step(&p.items, first.line, first.col, true, false).unwrap();
        assert_eq!(i, 1);
        assert_eq!(step(&p.items, first.line, first.col, false, false), None);
        assert_eq!(step(&p.items, first.line, first.col, false, true), Some(p.items.len() - 1));
        let s = section_step(&p.sections, first.line, true, false).unwrap();
        assert_eq!(s, p.sections[1].1);
        assert_eq!(section_step(&p.sections, p.sections[0].1, false, false), None);
        assert_eq!(item_at(&p.items, first.line), Some(0));
    }

    #[test]
    fn projects_come_from_recent_files_once_each() {
        let r = vec!["/p/a/x".to_string(), "/p/a/y".into(), "/p/b/z".into(), "/tmp/q".into()];
        let out = projects_of(&r, |f| f.strip_prefix("/p/").map(|s| format!("/p/{}", &s[..1])));
        assert_eq!(out, ["/p/a", "/p/b"]);
    }

    #[test]
    fn settings_parse_and_typos_are_errors() {
        assert_eq!(parse_config("{}").unwrap(), Config::default());
        let c = parse_config(
            r#"{"banner":2,"items":[["recents",3],"bookmarks"],"week_agenda":true,
                "navigator":[[["Home","browse-url"]]],"item_shortcuts":{"recents":"x"}}"#,
        )
        .unwrap();
        assert_eq!(c.banner, Banner::Numbered(2));
        assert_eq!(c.items, [("recents".into(), 3), ("bookmarks".into(), 20)]);
        assert!(c.week_agenda);
        assert_eq!(c.navigator[0][0].1, "browse-url");
        assert_eq!(c.item_shortcuts["recents"], "x");
        assert!(parse_config(r#"{"bannr":1}"#).is_err());
        assert!(parse_config(r#"{"banner":9}"#).is_err());
        assert!(parse_config(r#"{"items":["nope"]}"#).is_err());
        assert!(parse_config("[]").is_err());
        assert!(parse_config(r#"{"width":5}"#).is_err());
    }

    #[test]
    fn paths_are_abbreviated_and_long_ones_shortened() {
        assert_eq!(abbreviate("/home/me/x", "/home/me"), "~/x");
        assert_eq!(abbreviate("/home/meh/x", "/home/me"), "/home/meh/x");
        assert_eq!(shorten("short", 20), "short");
        let s = shorten(&"a".repeat(100), 30);
        assert_eq!(s.chars().count(), 30);
        assert!(s.contains('…'));
    }

    #[test]
    fn navigator_buttons_are_items_that_run_commands() {
        let mut c = Config::default();
        c.navigator = vec![vec![("Scratch".into(), "scratch-buffer".into())]];
        let p = build(&c, &Data::default(), "", "d", 0);
        let nav = p.items.iter().find(|i| i.section == "navigator").unwrap();
        assert_eq!(nav.action, Action::Command("scratch-buffer".into()));
        assert!(p.text.contains("[Scratch]"));
    }

    #[test]
    fn a_picture_banner_reserves_a_line() {
        let mut c = Config::default();
        c.banner = Banner::Path("/tmp/logo.png".into());
        let p = build(&c, &Data::default(), "", "d", 0);
        assert_eq!(p.image.as_ref().map(|i| i.1.as_str()), Some("/tmp/logo.png"));
    }

    #[test]
    fn today_is_a_plausible_iso_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }

    #[test]
    fn icons_mark_headings_and_items_and_dim_the_path() {
        let mut c = Config::default();
        c.banner = Banner::None;
        c.icons = Some(true);
        let p = build(&c, &data(), "/home/me", "d", 0);
        assert!(p.text.contains("\u{25D4} Recent Files:"));
        assert!(p.text.contains("\u{25AB} "));
        let mut plain = Config::default();
        plain.banner = Banner::None;
        plain.icons = Some(false);
        let q = build(&plain, &data(), "/home/me", "d", 0);
        assert!(!q.text.contains('\u{25D4}'));
        // Every item is still reachable at the same column.
        assert_eq!(p.items.len(), q.items.len());
    }

    #[test]
    fn selectable_things_are_links_and_lists_start_at_the_left_edge() {
        let mut c = Config::default();
        c.banner = Banner::None;
        c.navigator = vec![vec![("Homepage".into(), "x".into())]];
        let p = build(&c, &data(), "/home/me", "d", 0);
        // Every item (list entries and buttons) carries the link face.
        for it in &p.items {
            assert!(
                p.decos.iter().any(|d| d.line == it.line && d.start <= it.col && it.col < d.end && d.face == "link"),
                "item on line {} is not drawn as a link",
                it.line
            );
        }
        // Headings are at column 0; the title is centred.
        let heading = p.text.lines().find(|l| l.contains("Recent Files:")).unwrap();
        assert!(heading.starts_with("Recent Files:"));
        let title = p.text.lines().find(|l| l.contains("Welcome to Forge")).unwrap();
        assert!(title.starts_with(' '));
    }
}
