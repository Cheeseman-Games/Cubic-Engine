//! Bottom pane: retained engine/editor log plus a command line.

use egui::Key;
use egui::ScrollArea;

use crate::state::{EditorState, LogLevel};

/// Draws the console pane.
pub fn console(ui: &mut egui::Ui, state: &mut EditorState) {
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &state.console {
                let color = match line.level {
                    LogLevel::Info => ui.visuals().text_color(),
                    LogLevel::Warn => egui::Color32::from_rgb(0xe5, 0x9b, 0x3b),
                    LogLevel::Error => egui::Color32::from_rgb(0xef, 0x43, 0x43),
                };
                ui.label(egui::RichText::new(&line.text).color(color).monospace());
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(">");
        let edit = ui.text_edit_singleline(&mut state.console_input);
        let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if submitted {
            let command = state.console_input.trim().to_owned();
            if !command.is_empty() {
                state.log(LogLevel::Info, format!("> {command}"));
            }
            state.console_input.clear();
            edit.request_focus();
        }
    });
}
