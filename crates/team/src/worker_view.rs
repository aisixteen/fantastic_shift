//! Worker view (`team_worker_view.tools`): one vertical canvas per worker in
//! fantastic's `mobile` layout. The top part holds generated cells (task card,
//! files, output, preview) and takes the rest of the height; the worker's chat
//! is the last cell, locked at the bottom. The view stores only the worker id,
//! and its chat targets that id only.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use fantastic_kernel::bundle::{Bundle, BundleError, Reply};
use fantastic_kernel::{AgentId, Kernel};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The handler module name this bundle registers under.
pub const HANDLER_MODULE: &str = "team_worker_view.tools";
const NAME: &str = "team_worker_view";
/// The layout of fantastic's canvas this view uses (canvas | mobile).
pub const LAYOUT: &str = "mobile";
/// The fit rules on top of the mobile theme.
pub const CSS: &str = include_str!("../web/worker_view.css");
/// The group a view joins when `open` names none.
const DEFAULT_GROUP: &str = "workers";

/// One cell of a worker view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    /// What the cell shows: `task`, `files`, `output`, `preview`, or `chat`.
    pub kind: String,
    /// The heading shown on the cell.
    pub label: String,
    /// The agent the cell talks to (the chat cell only).
    pub target: Option<String>,
    /// Whether the cell is fixed in place (the chat cell only).
    pub locked: bool,
}

/// One worker's canvas.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// The worker agent id this view is bound to.
    pub worker: String,
    /// The group this view cycles within for `next` and `prev`.
    pub group: String,
    /// The fantastic layout it renders in.
    pub layout: String,
    /// Top cells first, then the locked chat cell last.
    pub cells: Vec<Cell>,
}

/// The default cell set for a worker: task, files, output, preview on top,
/// then the locked chat cell.
pub fn default_cells(worker: &str) -> Vec<Cell> {
    let top = [
        ("task", "Task"),
        ("files", "Files touched"),
        ("output", "Build and test output"),
        ("preview", "Result preview"),
    ];
    let mut out: Vec<Cell> = top
        .iter()
        .map(|(kind, label)| Cell {
            kind: kind.to_string(),
            label: label.to_string(),
            target: None,
            locked: false,
        })
        .collect();
    out.push(chat_cell(worker));
    out
}

/// The locked chat cell for a worker. It targets that worker only.
pub fn chat_cell(worker: &str) -> Cell {
    Cell {
        kind: "chat".to_string(),
        label: "Chat".to_string(),
        target: Some(worker.to_string()),
        locked: true,
    }
}

/// Add a cell to the top part only: it goes before the locked chat cell.
/// A cell may not be a chat cell or locked; those come from [`chat_cell`].
pub fn place(view: &mut View, mut cell: Cell) -> Result<(), String> {
    if cell.kind == "chat" {
        return Err("a chat cell is placed by the view, not by place".to_string());
    }
    cell.locked = false;
    let at = view.cells.len().saturating_sub(1);
    view.cells.insert(at, cell);
    Ok(())
}

/// The worker `step` places away from `worker` within its group, wrapping
/// around. Members are ordered by worker id. `None` when `worker` has no view.
pub(crate) fn cycle(views: &BTreeMap<String, View>, worker: &str, step: isize) -> Option<String> {
    let me = views.get(worker)?;
    let members: Vec<&str> = views
        .values()
        .filter(|v| v.group == me.group)
        .map(|v| v.worker.as_str())
        .collect();
    let i = members.iter().position(|w| *w == worker)? as isize;
    let n = members.len() as isize;
    Some(members[(i + step).rem_euclid(n) as usize].to_string())
}

fn arg<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}

/// The worker view bundle. Holds one view per worker id.
#[derive(Default)]
pub struct WorkerViewBundle {
    views: Mutex<BTreeMap<String, View>>,
}

#[async_trait::async_trait]
impl Bundle for WorkerViewBundle {
    fn name(&self) -> &str {
        NAME
    }

    fn verb_docs(&self) -> Value {
        json!({
            "open": "open {worker, group?} → view (creates it, or returns the existing one)",
            "cells": "cells {worker} → {cells} (the default cell set)",
            "place": "place {worker, cell: {kind, label}} → view (adds a cell to the top part)",
            "next": "next {worker} → {worker} (the next worker in its group)",
            "prev": "prev {worker} → {worker} (the previous worker in its group)",
        })
    }

    async fn handle(
        &self,
        _agent_id: &AgentId,
        payload: &Value,
        _kernel: &Arc<Kernel>,
    ) -> Result<Reply, BundleError> {
        let t = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let mut views = self.views.lock().expect("worker view state");
        let out = match t {
            "open" => match arg(payload, "worker") {
                None => json!({"error": "open needs worker"}),
                Some(w) => {
                    let group = arg(payload, "group").unwrap_or(DEFAULT_GROUP).to_string();
                    let view = views.entry(w.to_string()).or_insert_with(|| View {
                        worker: w.to_string(),
                        group,
                        layout: LAYOUT.to_string(),
                        cells: default_cells(w),
                    });
                    json!(view)
                }
            },
            "cells" => match arg(payload, "worker") {
                None => json!({"error": "cells needs worker"}),
                Some(w) => json!({"cells": default_cells(w)}),
            },
            "place" => {
                let worker = arg(payload, "worker").unwrap_or("");
                match (views.get_mut(worker), payload.get("cell")) {
                    (None, _) => json!({"error": format!("no view for '{worker}'; open it first")}),
                    (Some(_), None) => json!({"error": "place needs cell"}),
                    (Some(view), Some(c)) => match serde_json::from_value::<Cell>(c.clone()) {
                        Err(e) => json!({"error": format!("bad cell: {e}")}),
                        Ok(cell) => match place(view, cell) {
                            Ok(()) => json!(view),
                            Err(e) => json!({"error": e}),
                        },
                    },
                }
            }
            "next" | "prev" => {
                let step = if t == "next" { 1 } else { -1 };
                match arg(payload, "worker").and_then(|w| cycle(&views, w, step)) {
                    Some(w) => json!({"worker": w}),
                    None => json!({"error": "no view in this group for that worker"}),
                }
            }
            other => json!({"error": format!("team_worker_view: unknown type '{other}'")}),
        };
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(worker: &str, group: &str) -> View {
        View {
            worker: worker.into(),
            group: group.into(),
            layout: LAYOUT.into(),
            cells: default_cells(worker),
        }
    }

    #[test]
    fn open_binds_view_to_one_worker() {
        let v = view("w1", DEFAULT_GROUP);
        assert_eq!(v.worker, "w1");
        assert_eq!(v.layout, "mobile");
        assert_eq!(chat_cell("w1").target.as_deref(), Some("w1"));
    }

    #[test]
    fn open_is_idempotent() {
        let mut views = BTreeMap::new();
        for _ in 0..2 {
            views
                .entry("w1".to_string())
                .or_insert_with(|| view("w1", DEFAULT_GROUP));
        }
        assert_eq!(views.len(), 1);
    }

    #[test]
    fn default_cells_are_task_files_output_preview() {
        let kinds: Vec<String> = default_cells("w1").into_iter().map(|c| c.kind).collect();
        assert_eq!(kinds, ["task", "files", "output", "preview", "chat"]);
    }

    #[test]
    fn chat_cell_is_last_and_locked() {
        let cells = default_cells("w1");
        let last = cells.last().unwrap();
        assert_eq!(last.kind, "chat");
        assert!(last.locked);
        assert_eq!(last.target.as_deref(), Some("w1"));
    }

    #[test]
    fn place_adds_to_top_part_only() {
        let mut v = view("w1", DEFAULT_GROUP);
        let cell = Cell {
            kind: "note".into(),
            label: "Note".into(),
            target: None,
            locked: true,
        };
        place(&mut v, cell).unwrap();
        assert_eq!(v.cells.len(), 6);
        assert_eq!(v.cells[4].kind, "note");
        assert!(!v.cells[4].locked);
        assert_eq!(v.cells.last().unwrap().kind, "chat");
        let chat = Cell {
            kind: "chat".into(),
            label: "x".into(),
            target: None,
            locked: false,
        };
        assert!(place(&mut v, chat).is_err());
    }

    #[test]
    fn next_and_prev_cycle_within_group() {
        let mut views = BTreeMap::new();
        for (w, g) in [("a", "g1"), ("b", "g1"), ("c", "g1"), ("x", "g2")] {
            views.insert(w.to_string(), view(w, g));
        }
        assert_eq!(cycle(&views, "a", 1).as_deref(), Some("b"));
        assert_eq!(cycle(&views, "c", 1).as_deref(), Some("a"));
        assert_eq!(cycle(&views, "a", -1).as_deref(), Some("c"));
        assert_eq!(cycle(&views, "x", 1).as_deref(), Some("x"));
        assert_eq!(cycle(&views, "missing", 1), None);
    }

    #[test]
    fn css_has_no_horizontal_overflow_rules() {
        assert!(CSS.contains("overflow-x: hidden"));
        assert_eq!(CSS.matches("overflow-y: auto").count(), 1);
    }

    #[test]
    fn register_team_registers_worker_view() {
        let mut reg = fantastic_kernel::BundleRegistry::new();
        crate::register_team(&mut reg);
        let names: Vec<&str> = reg.catalog().map(|(n, _)| n).collect();
        assert!(names.contains(&HANDLER_MODULE));
    }
}
