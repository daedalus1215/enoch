//! Sorting activities into categories from the window: right-click menus on
//! activities and category headings (in the rail and the Stats breakdown),
//! and the small dialogs they open to name, rename or delete a category.

use eframe::egui::{self, Id, Key, Response, RichText, Ui};

use super::Action;

/// A dialog over the window, waiting for a name or a yes.
#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    /// Name a new category; `then_move` goes into it once it exists.
    New {
        text: String,
        then_move: Option<String>,
    },
    Rename {
        old: String,
        text: String,
    },
    Delete {
        name: String,
        /// How many activities it holds, to say what deleting does.
        members: usize,
    },
}

impl Dialog {
    pub fn new_category(then_move: Option<&str>) -> Self {
        Dialog::New {
            text: String::new(),
            then_move: then_move.map(str::to_string),
        }
    }
}

/// Right-click menu on an activity: move it to another category, out of
/// its category, or into a new one.
pub fn activity_menu(
    resp: &Response,
    name: &str,
    current: Option<&str>,
    categories: &[String],
    actions: &mut Vec<Action>,
) {
    resp.context_menu(|ui| {
        ui.menu_button("Move to", |ui| {
            let mut moved = |ui: &mut Ui, category: Option<&str>| {
                actions.push(Action::MoveActivity {
                    activity: name.to_string(),
                    category: category.map(str::to_string),
                });
                ui.close();
            };
            for c in categories {
                let here = current.is_some_and(|cur| cur.eq_ignore_ascii_case(c));
                if ui
                    .add_enabled(!here, egui::Button::new(c.as_str()))
                    .clicked()
                {
                    moved(ui, Some(c));
                }
            }
            if !categories.is_empty() {
                ui.separator();
            }
            if ui
                .add_enabled(current.is_some(), egui::Button::new("No category"))
                .clicked()
            {
                moved(ui, None);
            }
            if ui.button("New category…").clicked() {
                actions.push(Action::Dialog(Dialog::new_category(Some(name))));
                ui.close();
            }
        });
    });
}

/// Right-click menu on a category heading; `None` is the no-category group,
/// which can't be renamed or deleted.
pub fn category_menu(
    resp: &Response,
    name: Option<&str>,
    members: usize,
    actions: &mut Vec<Action>,
) {
    resp.context_menu(|ui| {
        if let Some(name) = name {
            if ui.button("Rename…").clicked() {
                actions.push(Action::Dialog(Dialog::Rename {
                    old: name.to_string(),
                    text: name.to_string(),
                }));
                ui.close();
            }
            if ui.button("Delete…").clicked() {
                actions.push(Action::Dialog(Dialog::Delete {
                    name: name.to_string(),
                    members,
                }));
                ui.close();
            }
            ui.separator();
        }
        if ui.button("New category…").clicked() {
            actions.push(Action::Dialog(Dialog::new_category(None)));
            ui.close();
        }
    });
}

/// Show the open dialog, if any; it closes on Cancel, Escape, a click
/// outside it, or once its action is sent.
pub fn show(ctx: &egui::Context, dialog: &mut Option<Dialog>, actions: &mut Vec<Action>) {
    let Some(d) = dialog else { return };
    let mut done = false;
    let modal = egui::Modal::new(Id::unique("category-dialog")).show(ctx, |ui| {
        ui.set_width(320.0);
        match d {
            Dialog::New { text, then_move } => {
                ui.heading("New category");
                if let Some(a) = then_move {
                    ui.weak(format!("{a} moves into it."));
                }
                ui.add_space(6.0);
                if let Some(name) = name_field(ui, text, "Create") {
                    actions.push(Action::CreateCategory {
                        name,
                        then_move: then_move.clone(),
                    });
                    done = true;
                }
            }
            Dialog::Rename { old, text } => {
                ui.heading(format!("Rename {old}"));
                ui.add_space(6.0);
                if let Some(new) = name_field(ui, text, "Rename") {
                    if new != *old {
                        actions.push(Action::RenameCategory {
                            old: old.clone(),
                            new,
                        });
                    }
                    done = true;
                }
            }
            Dialog::Delete { name, members } => {
                ui.heading(format!("Delete {name}?"));
                ui.label(match members {
                    0 => "It has no activities.".to_string(),
                    1 => "Its activity will have no category. No time is lost.".to_string(),
                    n => format!("Its {n} activities will have no category. No time is lost."),
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(RichText::new("Delete").color(super::style::ERROR))
                        .clicked()
                    {
                        actions.push(Action::DeleteCategory(name.clone()));
                        done = true;
                    }
                    if ui.button("Cancel").clicked() {
                        ui.close();
                    }
                });
            }
        }
    });
    if done || modal.should_close() {
        *dialog = None;
    }
}

/// A name box with a confirm and a Cancel button. Returns the trimmed name
/// once confirmed (button or Enter); Cancel closes the dialog.
fn name_field(ui: &mut Ui, text: &mut String, confirm: &str) -> Option<String> {
    let resp = ui.add(
        egui::TextEdit::singleline(text)
            .hint_text("name")
            .desired_width(f32::INFINITY),
    );
    // Check for Enter before taking focus back: `lost_focus` reads the
    // current focus, so refocusing first would hide the Enter.
    let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if !entered && ui.memory(|m| m.focused().is_none()) {
        resp.request_focus();
    }
    let name = text.trim();
    let mut confirmed = false;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        confirmed = ui
            .add_enabled(!name.is_empty(), egui::Button::new(confirm))
            .clicked();
        if ui.button("Cancel").clicked() {
            ui.close();
        }
    });
    ((confirmed || entered) && !name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    struct Open {
        dialog: Option<Dialog>,
        actions: Vec<Action>,
    }

    fn harness(d: Dialog) -> Harness<'static, Open> {
        Harness::builder()
            .with_size(egui::vec2(500.0, 400.0))
            .build_ui_state(
                |ui, s: &mut Open| show(ui.ctx(), &mut s.dialog, &mut s.actions),
                Open {
                    dialog: Some(d),
                    actions: Vec::new(),
                },
            )
    }

    #[test]
    fn new_category_is_named_and_made_on_enter() {
        let mut h = harness(Dialog::new_category(Some("guitar")));
        h.get_by_role(egui::Role::TextInput).type_text("  music ");
        h.run();
        h.key_press(Key::Enter);
        h.run();
        assert!(h.state().dialog.is_none());
        assert!(matches!(
            &h.state().actions[..],
            [Action::CreateCategory { name, then_move: Some(a) }] if name == "music" && a == "guitar"
        ));
    }

    #[test]
    fn renaming_to_the_same_name_does_nothing() {
        let mut h = harness(Dialog::Rename {
            old: "work".into(),
            text: "work".into(),
        });
        h.get_by_label("Rename").click();
        h.run();
        assert!(h.state().dialog.is_none());
        assert!(h.state().actions.is_empty());
    }

    #[test]
    fn delete_asks_first_and_cancel_backs_out() {
        let mut h = harness(Dialog::Delete {
            name: "work".into(),
            members: 2,
        });
        assert!(h.query_by_label_contains("Its 2 activities").is_some());
        h.get_by_label("Cancel").click();
        h.run();
        assert!(h.state().dialog.is_none());
        assert!(h.state().actions.is_empty());

        let mut h = harness(Dialog::Delete {
            name: "work".into(),
            members: 2,
        });
        h.get_by_label("Delete").click();
        h.run();
        assert!(matches!(&h.state().actions[..], [Action::DeleteCategory(n)] if n == "work"));
    }
}
