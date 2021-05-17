/*
 * This file is part of espanso.
 *
 * Copyright (C) 2019-2021 Federico Terzi
 *
 * espanso is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * espanso is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with espanso.  If not, see <https://www.gnu.org/licenses/>.
 */

use std::{path::Path, process::Command, time::Instant};

use crossbeam::{
  channel::{unbounded, Sender},
  select,
};
use espanso_ipc::IPCClient;
use espanso_path::Paths;
use log::{error, info, warn};

use crate::{
  ipc::{create_ipc_client_to_worker, IPCEvent},
  lock::{acquire_daemon_lock, acquire_worker_lock},
};

use super::{CliModule, CliModuleArgs};

mod ipc;

pub enum ExitCode {
  Success = 0,
  ExitCodeUnwrapError = 100,
}

pub fn new() -> CliModule {
  #[allow(clippy::needless_update)]
  CliModule {
    requires_paths: true,
    requires_config: true,
    enable_logs: true,
    log_mode: super::LogMode::CleanAndAppend,
    subcommand: "daemon".to_string(),
    entry: daemon_main,
    ..Default::default()
  }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn daemon_main(args: CliModuleArgs) -> i32 {
  let paths = args.paths.expect("missing paths in daemon main");

  // Make sure only one instance of the daemon is running
  let lock_file = acquire_daemon_lock(&paths.runtime);
  if lock_file.is_none() {
    error!("daemon is already running!");
    return 1;
  }

  // TODO: we might need to check preconditions: accessibility on macOS, presence of binaries on Linux, etc

  info!("espanso version: {}", VERSION);
  // TODO: print os system and version? (with os_info crate)

  let worker_ipc = create_ipc_client_to_worker(&paths.runtime)
    .expect("unable to create IPC client to worker process");

  terminate_worker_if_already_running(&paths.runtime, worker_ipc);

  let (exit_notify, exit_signal) = unbounded::<i32>();

  // TODO: register signals to terminate the worker if the daemon terminates

  spawn_worker(&paths, exit_notify.clone());

  ipc::initialize_and_spawn(&paths.runtime, exit_notify)
    .expect("unable to initialize ipc server for daemon");

  // TODO: start file watcher thread

  let mut exit_code: i32 = ExitCode::Success as i32;

  loop {
    select! {
      recv(exit_signal) -> code => {
        match code {
          Ok(code) => {
            exit_code = code
          },
          Err(err) => {
            error!("received error when unwrapping exit_code: {}", err);
            exit_code = ExitCode::ExitCodeUnwrapError as i32;
          },
        }
        break;
      },
    }
  }

  exit_code
}

fn terminate_worker_if_already_running(runtime_dir: &Path, worker_ipc: impl IPCClient<IPCEvent>) {
  let lock_file = acquire_worker_lock(&runtime_dir);
  if lock_file.is_some() {
    return;
  }

  warn!("a worker process is already running, sending termination signal...");
  if let Err(err) = worker_ipc.send(IPCEvent::Exit) {
    error!(
      "unable to send termination signal to worker process: {}",
      err
    );
  }

  let now = Instant::now();
  while now.elapsed() < std::time::Duration::from_secs(3) {
    let lock_file = acquire_worker_lock(runtime_dir);
    if lock_file.is_some() {
      return;
    }

    std::thread::sleep(std::time::Duration::from_millis(200));
  }

  panic!(
    "could not terminate worker process, please kill it manually, otherwise espanso won't start"
  )
}

fn spawn_worker(paths: &Paths, exit_notify: Sender<i32>) {
  info!("spawning the worker process...");

  let espanso_exe_path =
    std::env::current_exe().expect("unable to obtain espanso executable location");

  let mut command = Command::new(&espanso_exe_path.to_string_lossy().to_string());
  command.args(&["worker"]);
  command.env(
    "ESPANSO_CONFIG_DIR",
    paths.config.to_string_lossy().to_string(),
  );
  command.env(
    "ESPANSO_PACKAGE_DIR",
    paths.packages.to_string_lossy().to_string(),
  );
  command.env(
    "ESPANSO_RUNTIME_DIR",
    paths.runtime.to_string_lossy().to_string(),
  );

  // TODO: investigate if this is needed here, especially when invoking a form
  // // On windows, we need to spawn the process as "Detached"
  // #[cfg(target_os = "windows")]
  // {
  //   use std::os::windows::process::CommandExt;
  //   //command.creation_flags(0x08000008); // CREATE_NO_WINDOW + DETACHED_PROCESS
  // }

  let mut child = command.spawn().expect("unable to spawn worker process");

  // Create a monitor thread that will exit with the same non-zero code if
  // the worker thread exits
  std::thread::Builder::new()
    .name("worker-status-monitor".to_string())
    .spawn(move || {
      let result = child.wait();
      if let Ok(status) = result {
        if let Some(code) = status.code() {
          if code != 0 {
            error!(
              "worker process exited with non-zero code: {}, exiting",
              code
            );
            exit_notify
              .send(code)
              .expect("unable to forward worker exit code");
          }
        }
      }
    })
    .expect("Unable to spawn worker monitor thread");
}
