//! The transport bar: Play/Pause, Step and Stop for the open scene, plus the
//! Run controls that build and launch the project in its own process.
//!
//! The controls are a thin skin over [`crate::play`] and [`crate::run`]: each
//! button calls one of its helpers and reports the outcome to the console. The
//! readout to their right is the visible sign that a run is actually advancing
//! — the tick count and simulated seconds move under Play, stand still under
//! Pause.

use egui::{Button, Panel};

use crate::play;
use crate::run;
use crate::state::EditorState;

/// Draws the transport bar, docked under the menu bar.
pub fn toolbar(ui: &mut egui::Ui, state: &mut EditorState) {
    Panel::top("play_toolbar").show(ui, |ui| {
        ui.horizontal(|ui| {
            let playing = state.play.is_playing();
            let paused = state.play.is_paused();
            let active = state.play.is_active();

            let toggle_label = if playing {
                "Pause"
            } else if paused {
                "Resume"
            } else {
                "Play"
            };
            if ui
                .add(Button::new(toggle_label).shortcut_text("F5"))
                .clicked()
            {
                let outcome = play::toggle_play(state);
                state.report(outcome);
            }

            if ui
                .add_enabled(!playing, Button::new("Step").shortcut_text("F6"))
                .on_hover_text("Advance exactly one fixed step")
                .clicked()
            {
                let outcome = play::step_play(state);
                state.report(outcome);
            }

            if ui
                .add_enabled(active, Button::new("Stop").shortcut_text("F7"))
                .clicked()
            {
                let outcome = play::stop_play(state);
                state.report(outcome);
            }

            ui.separator();
            ui.label(format!(
                "{}  ·  {} ticks  ·  {:.2}s",
                state_name(playing, paused),
                state.play.steps(),
                state.play.elapsed(),
            ));

            // The placeholder Run: build the open project and launch it as its
            // own process, streaming cargo's output to the console. Later this
            // bar gains "host in this window" controls; the `cargo run` behind
            // it stays until then.
            ui.separator();
            let has_project = state.project.is_some();
            if ui
                .add_enabled(has_project && !state.run.is_running(), Button::new("Run"))
                .on_hover_text("Build and run the open project")
                .clicked()
            {
                let outcome = run::run_project(state);
                state.report(outcome);
            }
            if ui
                .add_enabled(state.run.is_running(), Button::new("Stop run"))
                .clicked()
            {
                let outcome = run::stop_run(state);
                state.report(outcome);
            }
        });
    });
}

/// The transport state as a word, from the flags the buttons already read.
fn state_name(playing: bool, paused: bool) -> &'static str {
    if playing {
        "playing"
    } else if paused {
        "paused"
    } else {
        "stopped"
    }
}
