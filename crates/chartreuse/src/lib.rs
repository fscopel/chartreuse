//! Chartreuse: a screenshot and annotation tool that lives in the menu bar.
//!
//! The app is an [`iced::daemon`]: it runs with no windows and opens them on
//! demand. See [`app`] for how state, messages, and windows are organized,
//! [`cli`] for the command line, and [`ipc`] for how one instance serves every
//! command-line invocation.
//!
//! The app is a library plus a thin `main.rs`, so the cross-module contracts
//! defined here ahead of their users are checked without dead-code noise.

pub mod alert;
pub mod app;
pub mod capture;
pub mod cli;
pub mod editor;
pub mod events;
pub mod export;
pub mod hotkeys;
pub mod import;
pub mod ipc;
pub mod overlay;
pub mod permission;
pub mod settings;
pub mod theme;
pub mod tray;
pub mod windows;

use std::cell::Cell;
use std::ops::ControlFlow;
use std::process::ExitCode;

use chartreuse_core::flavor;
use chartreuse_platform::EventReceiver;
use tracing_subscriber::EnvFilter;

/// The log filter when `RUST_LOG` is unset: our own logs at `info`, and only
/// warnings from the chatty graphics stack.
const DEFAULT_LOG_FILTER: &str =
    "info,wgpu_core=warn,wgpu_hal=warn,naga=warn,iced_wgpu=warn,iced_winit=warn";

/// Runs the command line ([`cli`]): hands its command to the running
/// instance, or becomes the instance and runs the daemon until the app exits.
/// Returns the process's exit status.
#[must_use]
pub fn run() -> ExitCode {
    let command = match cli::Cli::from_env().command() {
        Ok(command) => command,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER)),
        )
        .with_writer(std::io::stderr)
        .init();

    match single_instance(command.as_ref()) {
        ControlFlow::Continue(requests) => run_app(ipc::State::new(requests, command)),
        ControlFlow::Break(status) => status,
    }
}

/// Becomes the instance and serves its channel (`Continue`, with the requests;
/// `None` if there is no channel), or hands `command` to the running instance
/// (`Break`, with the exit status). Without a working channel the app still
/// starts, as a second instance if need be.
fn single_instance(
    command: Option<&ipc::Command>,
) -> ControlFlow<ExitCode, Option<EventReceiver<ipc::Request>>> {
    let endpoint = match ipc::Endpoint::current() {
        Ok(endpoint) => endpoint,
        Err(error) => {
            tracing::warn!(%error, "no channel to a running instance; starting anyway");
            return ControlFlow::Continue(None);
        }
    };
    match ipc::connect_or_claim(&endpoint) {
        Ok(ipc::Role::Client(stream)) => ControlFlow::Break(forward(stream, command)),
        Ok(ipc::Role::Instance(listener)) => match ipc::serve(listener) {
            Ok(requests) => {
                tracing::info!(%endpoint, "listening for commands");
                ControlFlow::Continue(Some(requests))
            }
            Err(error) => {
                tracing::warn!(%error, "cannot listen for commands; starting anyway");
                ControlFlow::Continue(None)
            }
        },
        Err(error) => {
            tracing::warn!(%error, %endpoint, "cannot reach a running instance; starting anyway");
            ControlFlow::Continue(None)
        }
    }
}

/// Hands `command` to the running instance over `stream`; the exit status.
fn forward(stream: ipc::Stream, command: Option<&ipc::Command>) -> ExitCode {
    let Some(command) = command else {
        eprintln!("{} is already running", flavor::DISPLAY_NAME);
        return ExitCode::SUCCESS;
    };
    match ipc::send(stream, command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the daemon until the app exits.
fn run_app(ipc: ipc::State) -> ExitCode {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        bundle_id = flavor::BUNDLE_ID,
        "starting {}",
        flavor::DISPLAY_NAME
    );
    #[cfg(windows)]
    leave_own_console();

    // iced boots once; the cell hands the state over by value.
    let ipc = Cell::new(Some(ipc));
    let result = iced::daemon(
        move || app::App::boot(ipc.take().unwrap_or_default()),
        app::App::update,
        app::App::view,
    )
    .title(app::App::title)
    .theme(app::App::theme)
    .subscription(app::App::subscription)
    .run();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "{} failed", flavor::DISPLAY_NAME);
            ExitCode::FAILURE
        }
    }
}

/// Closes the console window Windows opens for a console program started
/// outside a terminal (from Explorer, a shortcut, or a login item), which
/// would otherwise stay open behind the tray icon until the app quits. From a
/// terminal the console is shared with the shell, so it stays, and the
/// instance keeps logging there.
///
/// Chartreuse stays a console program so that the command line can report to
/// the terminal and its exit status is waited for.
#[cfg(windows)]
fn leave_own_console() {
    use ::windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

    // Room for one more process than this one: enough to tell alone from shared.
    let mut processes = [0; 2];
    // SAFETY: the buffer is valid for its length, which the binding passes.
    let attached = unsafe { GetConsoleProcessList(&mut processes) };
    if attached == 1 {
        // SAFETY: detaching only invalidates the standard handles, whose
        // writes (the logs) then fail quietly.
        if let Err(error) = unsafe { FreeConsole() } {
            tracing::debug!(%error, "could not close the console window");
        }
    }
}
