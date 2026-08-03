//! Standalone Wayland host for the aegis System Settings modules.

use std::collections::VecDeque;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use aegis_core::settings::{SettingsAction, SettingsSnapshot};
use aegis_design::{Design, themes};
use aegis_ipc::{Client, ConnectionCapabilities};
use aegis_settings::builtin_settings_modules;
use aegis_settings::module::{ModuleAvailability, ModuleEvents, ModuleId, ModuleRegistry};
use aegis_shell::{Localizer, Message};
use iris::{Application, Config, Input, PaintHost, request_animation_frame};
use lens::{Align, Color, Frame, Icon, LayoutOpts};

const IPC_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
enum WorkerCommand {
    Load,
    Apply {
        expected_revision: u64,
        action: SettingsAction,
    },
}

#[derive(Debug)]
enum WorkerEvent {
    Snapshot(SettingsSnapshot),
    Failed {
        message: String,
        snapshot: Option<SettingsSnapshot>,
    },
}

#[derive(Debug)]
struct WorkerFailure {
    message: String,
    snapshot: Option<SettingsSnapshot>,
}

fn worker_failure(
    message: impl Into<String>,
    snapshot: Option<SettingsSnapshot>,
) -> Box<WorkerFailure> {
    Box::new(WorkerFailure {
        message: message.into(),
        snapshot,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    Loading,
    Applying,
}

struct SettingsApp {
    modules: ModuleRegistry,
    selected: ModuleId,
    i18n: Localizer,
    snapshot: Option<SettingsSnapshot>,
    queued: VecDeque<SettingsAction>,
    busy: Option<Busy>,
    error: Option<String>,
    worker_tx: Sender<WorkerCommand>,
    worker_rx: Receiver<WorkerEvent>,
}

impl SettingsApp {
    fn new(requested_module: Option<&str>, socket: Result<PathBuf, String>) -> Self {
        let modules = builtin_settings_modules();
        let selected = requested_module
            .and_then(|id| modules.resolve(id))
            .or_else(|| modules.metadata().next().map(|module| module.id))
            .expect("the built-in settings registry must not be empty");
        let (worker_tx, command_rx) = mpsc::channel();
        let (event_tx, worker_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("aegis-settings-ipc".into())
            .spawn(move || worker_loop(socket, command_rx, event_tx))
            .expect("spawn System Settings IPC worker");

        let mut app = Self {
            modules,
            selected,
            i18n: Localizer::from_env(),
            snapshot: None,
            queued: VecDeque::new(),
            busy: None,
            error: None,
            worker_tx,
            worker_rx,
        };
        app.load();
        app
    }

    fn load(&mut self) {
        if self.busy.is_some() {
            return;
        }
        self.error = None;
        self.busy = Some(Busy::Loading);
        if self.worker_tx.send(WorkerCommand::Load).is_err() {
            self.busy = None;
            self.error = Some("the settings IPC worker stopped".into());
        }
    }

    fn poll_worker(&mut self) {
        while let Ok(event) = self.worker_rx.try_recv() {
            match event {
                WorkerEvent::Snapshot(snapshot) => {
                    self.modules.update_settings(&snapshot);
                    self.snapshot = Some(snapshot);
                    self.busy = None;
                    self.error = None;
                }
                WorkerEvent::Failed { message, snapshot } => {
                    if let Some(snapshot) = snapshot {
                        self.modules.update_settings(&snapshot);
                        self.snapshot = Some(snapshot);
                    }
                    self.busy = None;
                    self.error = Some(message);
                    self.queued.clear();
                }
            }
        }
        self.start_queued_action();
    }

    fn enqueue(&mut self, action: SettingsAction) {
        // Instant controls may emit several drafts while a previous value is
        // in flight. Keep only the newest unsent action of each module kind.
        self.queued
            .retain(|queued| !same_action_kind(queued, &action));
        self.queued.push_back(action);
        self.start_queued_action();
    }

    fn start_queued_action(&mut self) {
        if self.busy.is_some() || self.error.is_some() {
            return;
        }
        let (Some(snapshot), Some(action)) = (self.snapshot.as_ref(), self.queued.pop_front())
        else {
            return;
        };
        let command = WorkerCommand::Apply {
            expected_revision: snapshot.revision,
            action,
        };
        if self.worker_tx.send(command).is_ok() {
            self.busy = Some(Busy::Applying);
        } else {
            self.error = Some("the settings IPC worker stopped".into());
        }
    }

    fn render(&mut self, frame: &mut Frame, input: &Input) {
        self.poll_worker();
        frame.set_theme(themes::application(&Design::dark()));
        let raw = input.as_raw();
        let width = raw.display_size.x.max(1.0);
        let height = raw.display_size.y.max(1.0);

        let mut module_events = ModuleEvents::default();
        frame.column_ex(
            &LayoutOpts {
                width,
                height,
                gap: 12.0,
                pad: 22.0,
                cross: Align::Stretch,
                bg: Color::rgba(25, 28, 40, 255),
                ..Default::default()
            },
            |frame| {
                self.render_header(frame);
                frame.separator();
                self.render_status(frame);
                frame.flex(1.0);
                if self.snapshot.is_none() {
                    self.render_empty_state(frame);
                } else if width >= 640.0 {
                    frame.row_ex(
                        &LayoutOpts {
                            flex: 1.0,
                            gap: 18.0,
                            cross: Align::Stretch,
                            ..Default::default()
                        },
                        |frame| {
                            self.render_sidebar(frame);
                            frame.flex(1.0);
                            frame.scroll("standalone-settings-page", |frame| {
                                frame.column_ex(
                                    &LayoutOpts {
                                        gap: 12.0,
                                        cross: Align::Stretch,
                                        ..Default::default()
                                    },
                                    |frame| {
                                        let _ = self.modules.render(
                                            self.selected,
                                            frame,
                                            &self.i18n,
                                            &mut module_events,
                                        );
                                    },
                                );
                            });
                        },
                    );
                } else {
                    self.render_compact_navigation(frame);
                    frame.flex(1.0);
                    frame.scroll("standalone-settings-narrow-page", |frame| {
                        frame.column_ex(
                            &LayoutOpts {
                                gap: 12.0,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |frame| {
                                let _ = self.modules.render(
                                    self.selected,
                                    frame,
                                    &self.i18n,
                                    &mut module_events,
                                );
                            },
                        );
                    });
                }
            },
        );

        for action in module_events.actions {
            self.enqueue(action);
        }
        if self.busy.is_some() {
            // Keep polling the background worker without forcing the app to
            // render continuously once the transaction completes.
            request_animation_frame();
        }
    }

    fn render_header(&mut self, frame: &mut Frame) {
        frame.row_ex(
            &LayoutOpts {
                height: 46.0,
                gap: 12.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.icon(Icon::Settings, 28.0);
                frame.column_ex(
                    &LayoutOpts {
                        gap: 1.0,
                        cross: Align::Start,
                        ..Default::default()
                    },
                    |frame| {
                        frame.heading(self.i18n.text(Message::SystemSettings), 2);
                        frame.label_sized(self.i18n.text(Message::StandaloneSettingsApp), 11.0);
                    },
                );
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.size_next(94.0, 30.0);
                if frame.button(self.i18n.text(Message::Refresh)) {
                    self.load();
                }
            },
        );
    }

    fn render_status(&mut self, frame: &mut Frame) {
        if let Some(busy) = self.busy {
            let message = match busy {
                Busy::Loading => Message::ConnectingToDesktop,
                Busy::Applying => Message::SavingSettings,
            };
            frame.label_sized(self.i18n.text(message), 11.0);
        }
        if let Some(error) = self.error.clone() {
            frame.column_ex(
                &LayoutOpts {
                    gap: 5.0,
                    pad: 10.0,
                    cross: Align::Stretch,
                    bg: Color::rgba(116, 43, 54, 178),
                    radius: 10.0,
                    ..Default::default()
                },
                |frame| {
                    frame.label_wrapped_sized(
                        self.i18n.text(Message::SettingsConnectionFailed),
                        11.0,
                        620.0,
                    );
                    frame.label_wrapped_sized(&error, 10.0, 620.0);
                    frame.size_next(94.0, 28.0);
                    if frame.button(self.i18n.text(Message::Retry)) {
                        self.load();
                    }
                },
            );
        }
    }

    fn render_empty_state(&self, frame: &mut Frame) {
        frame.flex(1.0);
        frame.column_ex(
            &LayoutOpts {
                gap: 8.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.icon(Icon::Settings, 38.0);
                frame.label_sized(self.i18n.text(Message::ConnectingToDesktop), 12.0);
            },
        );
        frame.flex(1.0);
        frame.spacer(0.0);
    }

    fn render_sidebar(&mut self, frame: &mut Frame) {
        let modules = self.modules.metadata().collect::<Vec<_>>();
        frame.column_ex(
            &LayoutOpts {
                width: 194.0,
                gap: 5.0,
                pad: 8.0,
                cross: Align::Stretch,
                bg: Color::rgba(255, 255, 255, 10),
                radius: 14.0,
                ..Default::default()
            },
            |frame| {
                for module in modules {
                    let label = module_label(&self.i18n, module);
                    if frame.selectable_icon(module.icon, &label, self.selected == module.id) {
                        self.selected = module.id;
                    }
                }
            },
        );
    }

    fn render_compact_navigation(&mut self, frame: &mut Frame) {
        let modules = self.modules.metadata().collect::<Vec<_>>();
        for row in modules.chunks(2) {
            frame.row_ex(
                &LayoutOpts {
                    gap: 5.0,
                    cross: Align::Stretch,
                    ..Default::default()
                },
                |frame| {
                    for module in row.iter().copied() {
                        frame.flex(1.0);
                        let label = module_label(&self.i18n, module);
                        if frame.selectable(&label, self.selected == module.id) {
                            self.selected = module.id;
                        }
                    }
                    if row.len() == 1 {
                        frame.flex(1.0);
                        frame.spacer(0.0);
                    }
                },
            );
        }
    }
}

fn module_label(i18n: &Localizer, module: aegis_settings::module::ModuleMetadata) -> String {
    let title = i18n.text(module.title);
    match module.availability {
        ModuleAvailability::Available => title.to_owned(),
        ModuleAvailability::BackendUnavailable => {
            format!("{title} · {}", i18n.text(Message::Unavailable))
        }
    }
}

fn same_action_kind(left: &SettingsAction, right: &SettingsAction) -> bool {
    matches!(
        (left, right),
        (
            SettingsAction::SetTouchpad { .. },
            SettingsAction::SetTouchpad { .. }
        ) | (
            SettingsAction::SetDisplay { .. },
            SettingsAction::SetDisplay { .. }
        ) | (
            SettingsAction::SetDesktopPreferences { .. },
            SettingsAction::SetDesktopPreferences { .. }
        ) | (
            SettingsAction::SetIdle { .. },
            SettingsAction::SetIdle { .. }
        )
    )
}

fn worker_loop(
    socket: Result<PathBuf, String>,
    commands: Receiver<WorkerCommand>,
    events: Sender<WorkerEvent>,
) {
    while let Ok(command) = commands.recv() {
        let event = match &socket {
            Ok(socket) => match execute_command(socket, command) {
                Ok(snapshot) => WorkerEvent::Snapshot(snapshot),
                Err(failure) => WorkerEvent::Failed {
                    message: failure.message,
                    snapshot: failure.snapshot,
                },
            },
            Err(message) => WorkerEvent::Failed {
                message: message.clone(),
                snapshot: None,
            },
        };
        if events.send(event).is_err() {
            break;
        }
    }
}

fn execute_command(
    socket: &Path,
    command: WorkerCommand,
) -> Result<SettingsSnapshot, Box<WorkerFailure>> {
    let requested = match &command {
        WorkerCommand::Load => ConnectionCapabilities::QUERY,
        WorkerCommand::Apply { .. } => ConnectionCapabilities {
            query: true,
            session: true,
            ..ConnectionCapabilities::default()
        },
    };
    let mut client = Client::connect_with_timeout(socket, requested, IPC_TIMEOUT)
        .map_err(|error| worker_failure(format!("{}: {error}", socket.display()), None))?;
    client
        .set_io_timeout(Some(IPC_TIMEOUT))
        .map_err(|error| worker_failure(error.to_string(), None))?;

    match command {
        WorkerCommand::Load => client
            .settings()
            .map_err(|error| worker_failure(error.to_string(), None)),
        WorkerCommand::Apply {
            expected_revision,
            action,
        } => {
            if !client.caps().session {
                return Err(worker_failure(
                    "the compositor did not grant the session capability",
                    None,
                ));
            }
            if let Err(error) = client.apply_settings(Some(expected_revision), action) {
                let snapshot = client.settings().ok();
                return Err(worker_failure(error.to_string(), snapshot));
            }
            client.settings().map_err(|error| {
                worker_failure(
                    format!("setting applied, but refresh failed: {error}"),
                    None,
                )
            })
        }
    }
}

fn socket_path() -> Result<PathBuf, String> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|runtime| runtime.join("aegis.sock"))
        .ok_or_else(|| "$XDG_RUNTIME_DIR is not set".to_owned())
}

fn requested_module(args: impl IntoIterator<Item = String>) -> Option<String> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--module=") {
            return Some(value.to_owned());
        }
        if arg == "--module" {
            return args.next();
        }
        if !arg.starts_with('-') {
            return Some(arg);
        }
    }
    None
}

fn main() -> Result<(), Box<dyn Error>> {
    aegis_logging::init("info");
    let requested = requested_module(std::env::args().skip(1));
    let mut app = SettingsApp::new(requested.as_deref(), socket_path());
    let config = Config::new(app.i18n.text(Message::SystemSettings))?
        .app_id(aegis_core::app::SETTINGS_APP_ID)?
        .size(920, 680)
        .force_dark();
    Application::run::<_, fn(PaintHost)>(
        config,
        move |frame, input| app.render(frame, input),
        None,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_argument_accepts_explicit_and_deep_link_forms() {
        assert_eq!(
            requested_module(["--module=display".into()]),
            Some("display".into())
        );
        assert_eq!(
            requested_module(["--module".into(), "touchpad".into()]),
            Some("touchpad".into())
        );
        assert_eq!(requested_module(["display".into()]), Some("display".into()));
    }

    #[test]
    fn action_kind_coalescing_keeps_modules_independent() {
        let touchpad = SettingsAction::SetTouchpad {
            config: Default::default(),
        };
        assert!(same_action_kind(&touchpad, &touchpad));
    }
}
