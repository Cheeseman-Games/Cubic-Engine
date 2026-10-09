//! Running the open project as a separate process, and streaming its output
//! into the Console.
//!
//! This is the editor's "Run" — a placeholder for hosting a project's game
//! *in-process*, which is the eventual goal. Until then the honest way to run a
//! project is the way `cubic-cli run` does it: `cargo run` on the project's own
//! manifest, so the game gets its own window and its own process, and the
//! editor gets cargo's build output back through pipes instead of inheriting a
//! terminal it does not have.
//!
//! Only one run is live at a time. The child's stdout/stderr are read on
//! background threads into a shared buffer that [`RunState::take_output`] hands
//! to the Console once a frame, and [`RunState::stop`] tears the process tree
//! down (cargo *and* the game it spawned) rather than orphaning the game window.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use cubic_cli::Project;
use cubic_cli::cargo::{self, Action};

use crate::state::{EditorState, LogLevel};

/// Lines read from a running process, drained into the Console each frame.
type Lines = Arc<Mutex<VecDeque<(LogLevel, String)>>>;

/// Cap on buffered lines between drains, so a noisy build cannot grow without
/// bound if the editor is not repainting.
const MAX_BUFFERED: usize = 2000;

/// One project process and the threads reading its output.
struct ProjectRun {
    child: Child,
    lines: Lines,
    readers: Vec<JoinHandle<()>>,
    /// The project's name, for the Console lines about it.
    title: String,
}

/// The editor's single external run, if any.
#[derive(Default)]
pub struct RunState {
    current: Option<ProjectRun>,
}

impl RunState {
    /// Whether a project process is live.
    pub fn is_running(&self) -> bool {
        self.current.is_some()
    }

    /// The running project's name, for UI that wants to say what is running.
    pub fn title(&self) -> Option<&str> {
        self.current.as_ref().map(|run| run.title.as_str())
    }

    /// Everything the process has printed since the last call, including the
    /// line announcing its exit.
    ///
    /// Called once a frame: while the process runs it moves the buffered lines
    /// out; when it has exited it joins the readers, drains the rest, and
    /// reports the exit status.
    pub fn take_output(&mut self) -> Vec<(LogLevel, String)> {
        let mut out = Vec::new();
        let Some(mut run) = self.current.take() else {
            return out;
        };

        let exit = match run.child.try_wait() {
            Ok(Some(status)) => Some(Ok(status)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        };
        if exit.is_none() {
            // Still going: hand out what is buffered and keep the run live.
            buffer_lines(&run.lines, &mut out);
            self.current = Some(run);
            return out;
        }

        // Exited: the readers see EOF and finish, then the last lines are safe
        // to take.
        for reader in run.readers.drain(..) {
            let _ = reader.join();
        }
        buffer_lines(&run.lines, &mut out);
        match exit.expect("checked for an exit above") {
            Ok(status) if status.success() => {
                out.push((LogLevel::Info, format!("`{}` exited ({status})", run.title)));
            }
            Ok(status) => {
                out.push((
                    LogLevel::Error,
                    format!("`{}` failed ({status})", run.title),
                ));
            }
            Err(error) => out.push((
                LogLevel::Error,
                format!("`{}` could not be polled: {error}", run.title),
            )),
        }
        out
    }

    /// Build and launch `project` as `cargo run`, refusing if one is already
    /// live or the project has nothing to build.
    pub fn start(&mut self, project: &Project, args: &[String]) -> Result<String, String> {
        if self.is_running() {
            return Err("a project is already running — stop it first".to_owned());
        }
        if !project.cargo_manifest().is_file() {
            return Err(format!(
                "{} has no Cargo.toml, so it cannot be built",
                project.root.display()
            ));
        }

        let title = project.manifest.game.name.clone();
        let mut command = Command::new(cargo::executable());
        command.args(cargo::argv(project, Action::Run, false, args));
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Cargo has no terminal to draw into here; without this it would
            // flash its own console window. The game still opens its own.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command
            .spawn()
            .map_err(|error| format!("could not start cargo: {error}"))?;
        let lines: Lines = Arc::new(Mutex::new(VecDeque::new()));
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(read_into(stdout, Arc::clone(&lines), LogLevel::Info));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(read_into(stderr, Arc::clone(&lines), LogLevel::Warn));
        }

        self.current = Some(ProjectRun {
            child,
            lines,
            readers,
            title: title.clone(),
        });
        Ok(format!("building and running `{title}`"))
    }

    /// Kill the running project (and whatever it spawned) and forget it.
    pub fn stop(&mut self) -> Result<String, String> {
        let Some(mut run) = self.current.take() else {
            return Err("nothing is running".to_owned());
        };
        kill_tree(&mut run.child);
        for reader in run.readers.drain(..) {
            let _ = reader.join();
        }
        Ok(format!("stopped `{}`", run.title))
    }
}

impl Drop for RunState {
    fn drop(&mut self) {
        // The editor is closing with a game still live: take it down rather than
        // leaving an orphaned window behind.
        if let Some(mut run) = self.current.take() {
            kill_tree(&mut run.child);
        }
    }
}

/// Build and run the open project. `Err` is a refusal (no project, nothing to
/// build, one already running).
pub fn run_project(state: &mut EditorState) -> Result<String, String> {
    let project = state
        .project
        .clone()
        .ok_or_else(|| "no project is open — open one first".to_owned())?;
    state.run.start(&project, &[])
}

/// Stop the running project.
pub fn stop_run(state: &mut EditorState) -> Result<String, String> {
    state.run.stop()
}

/// Move the buffered lines out of a run into `out`.
fn buffer_lines(lines: &Lines, out: &mut Vec<(LogLevel, String)>) {
    let mut buffer = lines.lock().expect("run buffer poisoned");
    out.extend(buffer.drain(..));
}

/// A thread that reads a child stream, line by line, into the shared buffer.
fn read_into<R: Read + Send + 'static>(source: R, lines: Lines, level: LogLevel) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(source).lines() {
            let Ok(line) = line else {
                break;
            };
            let mut buffer = lines.lock().expect("run buffer poisoned");
            buffer.push_back((level, line));
            while buffer.len() > MAX_BUFFERED {
                buffer.pop_front();
            }
        }
    })
}

/// Kill a child and its descendants, then reap it.
///
/// `cargo run` does not exit until the game it launched does, so killing only
/// cargo would orphan the game window. Asking the OS to end the process *tree*
/// takes both down.
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let pid = child.id().to_string();
        let _ = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::Scratch;

    #[test]
    fn starting_without_a_cargo_manifest_is_refused() {
        let scratch = Scratch::new("run-no-cargo");
        std::fs::write(scratch.join("game.toml"), "[game]\nname = \"demo\"\n").expect("manifest");
        let project = Project::open(&scratch.path).expect("a project, even without a Cargo.toml");

        let mut run = RunState::default();
        let error = run.start(&project, &[]).unwrap_err();
        assert!(error.contains("Cargo.toml"), "{error}");
        assert!(!run.is_running());
    }

    #[test]
    fn stopping_nothing_is_refused() {
        let mut run = RunState::default();
        assert!(run.stop().is_err());
    }

    #[test]
    fn running_without_an_open_project_is_refused() {
        let mut state = EditorState::new();
        assert!(run_project(&mut state).is_err());
        assert!(!state.run.is_running());
    }
}
