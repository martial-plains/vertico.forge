// SPDX-License-Identifier: GPL-3.0-or-later
//! vertico for Forge: every prompt with candidates (M-x, C-x b, C-x C-f,
//! plugin questions) lists them vertically under the prompt, filtered as
//! you type. C-n / C-p (or the arrows) move, RET picks, TAB inserts the
//! selected candidate, M-RET takes the input as typed.
//!
//! With it: orderless matching (space-separated parts in any order, a
//! part starting with `!` excludes) and marginalia-style annotations: a
//! command's key and summary in M-x, a buffer's mode, size and file in
//! C-x b. File candidates (C-x C-f) are annotated by the host itself with
//! permissions, size and age, since a plugin can't stat the files.
//!
//! Forge draws and filters the list (a plugin never runs on each key);
//! this plugin chooses how it looks and matches, and writes the
//! annotations once per list.

use forge_plugin_sdk::{
    Buffer, annotate, bind_in_prompt, command, commands, completion, first, forge_plugin, keymap,
    last, message, modes, next, previous, scroll, set_sorting,
};
use std::cell::{Cell, RefCell};

/// The prompt map the vertical list's keys live in, so they can be rebound.
const LIST: &str = "completion-list";

/// Vertico's default `vertico-count`.
const DEFAULT_LINES: u32 = 10;

/// The default sorting style (`vertico-sort-function`).
const SORT: &str = "history-length";

/// Every style `vertico-sort` will accept, in the order `M-x vertico-sort`
/// cycles them.
const SORTS: &[&str] = &["history-length", "history", "length", "alpha", "position"];

thread_local! {
    static ON: Cell<bool> = const { Cell::new(true) };
    static LINES: Cell<u32> = const { Cell::new(DEFAULT_LINES) };
    static CYCLE: Cell<bool> = const { Cell::new(false) };
    static SORTING: RefCell<String> = RefCell::new(SORT.to_string());
}

fn enable(on: bool) -> Result<(), String> {
    completion::set_minibuffer_ui(on, count());
    completion::set_minibuffer_cycle(ON.with(Cell::get) && CYCLE.with(Cell::get));
    let styles: Vec<String> = if on {
        vec!["orderless".into(), "flex".into()]
    } else {
        vec!["prefix".into()]
    };
    completion::set_matching(&styles)?;
    // Order matches after filtering: how often you pick something is the
    // most useful signal in M-x, and length next (vertico's default
    // sort, vertico-sort-history-length-alpha). Off, the list keeps the
    // order the prompt produced.
    let style = if on {
        SORTING.with(|s| s.borrow().clone())
    } else {
        "position".to_string()
    };
    set_sorting(&[style.as_str()]);
    Ok(())
}

/// How many rows the list shows (`vertico-count`).
fn count() -> u32 {
    LINES.with(Cell::get).clamp(1, 50)
}

/// The first sentence of a docstring, at most 60 characters.
pub fn summary(doc: &str) -> String {
    let first = doc.lines().next().unwrap_or("");
    let sentence = match first.find(". ") {
        Some(i) => &first[..=i],
        None => first,
    };
    if sentence.chars().count() > 60 {
        format!("{}...", sentence.chars().take(57).collect::<String>())
    } else {
        sentence.to_string()
    }
}

/// `/home/me/x` -> `~/x`.
pub fn tilde(path: &str) -> String {
    for home in ["/home/", "/Users/"] {
        if let Some(rest) = path.strip_prefix(home)
            && let Some(i) = rest.find('/')
        {
            return format!("~{}", &rest[i..]);
        }
    }
    match path.strip_prefix("/root") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn command_note(name: &str) -> String {
    let key = keymap::where_is(name)
        .into_iter()
        .next()
        .map(|k| format!("({k})  "))
        .unwrap_or_default();
    format!("{key}{}", summary(&commands::doc(name).unwrap_or_default()))
}

fn buffer_note(name: &str) -> String {
    let Some(b) = Buffer::find(name) else {
        return String::new();
    };
    let mode = b.major_mode();
    let mode = mode.strip_suffix("-mode").unwrap_or(&mode);
    let modified = if b.modified() { "* " } else { "" };
    let path = b.path().map(|p| tilde(&p)).unwrap_or_default();
    format!("{mode:<12} {:>6} lines  {modified}{path}", b.line_count())
}

/// `symbol` candidates come from imenu, which already names the file
/// after the symbol; nothing useful to add without re-finding the source.
fn symbol_note(_name: &str) -> String {
    String::new()
}

/// `package` and `plugin` candidates: no extra information the prompt
/// doesn't already show.
fn plain_note(_name: &str) -> String {
    String::new()
}

fn init() -> Result<(), String> {
    annotate("command", |names| {
        names.iter().map(|n| command_note(n)).collect()
    });
    annotate("buffer", |names| {
        names.iter().map(|n| buffer_note(n)).collect()
    });
    annotate("symbol", |names| {
        names.iter().map(|n| symbol_note(n)).collect()
    });
    annotate("package", |names| {
        names.iter().map(|n| plain_note(n)).collect()
    });
    annotate("plugin", |names| {
        names.iter().map(|n| plain_note(n)).collect()
    });
    enable(true)?;
    modes::define_minor_mode("vertico-mode")?;
    modes::set_lighter("vertico-mode", "")?;
    command(
        "vertico-mode",
        "Turn vertical minibuffer completion on or off.",
        |_| {
            let on = !ON.with(Cell::get);
            ON.with(|c| c.set(on));
            enable(on)?;
            message(&format!(
                "Vertico mode {}",
                if on { "enabled" } else { "disabled" }
            ));
            Ok(())
        },
    )?;
    command(
        "vertico-count",
        "How many rows the vertical list shows (vertico-count). With a numeric prefix argument, set it to that.",
        |prefix| {
            if let Some(n) = prefix {
                set_count(n as u32);
            }
            message(&format!("Vertico shows {} rows", count()));
            Ok(())
        },
    )?;
    command(
        "vertico-sort",
        "Cycle how the vertical list orders its candidates (vertico-sort-function).",
        |_| {
            let next = SORTING.with(|s| {
                let cur = s.borrow().clone();
                let i = SORTS.iter().position(|n| *n == cur).unwrap_or(0);
                let next = SORTS[(i + 1) % SORTS.len()].to_string();
                *s.borrow_mut() = next.clone();
                next
            });

            enable(ON.with(Cell::get))?;
            message(&format!("Vertico sorts by {next}"));
            Ok(())
        },
    )?;
    command(
        "vertico-cycle",
        "Toggle whether the list wraps around at the ends (vertico-cycle).",
        |_| {
            let on = !CYCLE.with(Cell::get);
            CYCLE.with(|c| c.set(on));
            enable(ON.with(Cell::get))?;
            message(&format!(
                "Vertico cycle {}",
                if on { "enabled" } else { "disabled" }
            ));
            Ok(())
        },
    )?;
    // Moving in the list and taking it, thin wrappers over the host's own
    // controls (the same ones corfu uses for its popup). vertico-cycle
    // makes next/previous wrap at the ends.
    command("vertico-next", "Select the next candidate.", |_| {
        next(1);
        Ok(())
    })?;
    command("vertico-previous", "Select the previous candidate.", |_| {
        previous(1);
        Ok(())
    })?;
    command("vertico-first", "Select the first candidate (M-<).", |_| {
        first();
        Ok(())
    })?;
    command("vertico-last", "Select the last candidate (M->).", |_| {
        last();
        Ok(())
    })?;
    command(
        "vertico-scroll-up",
        "Select the candidate a page up.",
        |_| {
            scroll(true);
            Ok(())
        },
    )?;
    command(
        "vertico-scroll-down",
        "Select the candidate a page down.",
        |_| {
            scroll(false);
            Ok(())
        },
    )?;
    // vertico-map: M-< / M-> are beginning-of-buffer / end-of-buffer in a
    // buffer, remapped onto the list while a prompt is up.
    let _ = bind_in_prompt(LIST, "M-<", "vertico-first");
    let _ = bind_in_prompt(LIST, "M->", "vertico-last");
    Ok(())
}

/// Set the row count and apply it (`vertico-count`, with a prefix arg).
fn set_count(n: u32) {
    LINES.with(|c| c.set(n.clamp(1, 50)));
    let _ = enable(ON.with(Cell::get));
}

forge_plugin!(init);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_and_paths() {
        assert_eq!(
            summary("Kill a buffer. With C-u, ask.\nMore."),
            "Kill a buffer."
        );
        assert_eq!(summary(""), "");
        assert!(summary(&"x".repeat(80)).ends_with("..."));
        assert_eq!(tilde("/home/ada/src/x.rs"), "~/src/x.rs");
        assert_eq!(tilde("/Users/ada/x"), "~/x");
        assert_eq!(tilde("/root/x"), "~/x");
        assert_eq!(tilde("/etc/hosts"), "/etc/hosts");
    }

    #[test]
    fn every_sort_name_is_one_the_host_knows() {
        // `M-x vertico-sort` cycles these, and each has to be a name
        // completion.set-sorting accepts.
        assert_eq!(SORTS[0], SORT, "the default is the first one cycled to");
        for name in SORTS {
            assert!(
                forge_core::completion::Sort::from_name(name).is_ok(),
                "{name} isn't a sorting style the host knows"
            );
        }
        // And they come back in a different order after a full cycle.
        let mut seen = Vec::new();
        for _ in 0..SORTS.len() {
            let cur = SORTING.with(|s| s.borrow().clone());
            if seen.contains(&cur) {
                break;
            }
            let i = SORTS.iter().position(|n| *n == cur).unwrap();
            seen.push(cur);
            SORTING.with(|s| *s.borrow_mut() = SORTS[(i + 1) % SORTS.len()].to_string());
        }
        assert_eq!(seen.len(), SORTS.len(), "cycling covers every style");
        SORTING.with(|s| *s.borrow_mut() = SORT.to_string());
    }

    #[test]
    fn the_row_count_is_clamped_to_what_the_host_accepts() {
        assert_eq!(count(), DEFAULT_LINES);
        LINES.with(|c| c.set(0));
        assert_eq!(count(), 1, "at least one row");
        LINES.with(|c| c.set(1000));
        assert_eq!(count(), 50, "the host clamps at 50");
        LINES.with(|c| c.set(DEFAULT_LINES));
    }
}
