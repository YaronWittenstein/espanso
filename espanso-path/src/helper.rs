use std::path::PathBuf;

use crate::PathsV2;

pub fn extract_runtime(force_runtime_dir: PathsV2) -> PathBuf {
  let runtime_dir = if let Some(runtime_dir) = force_runtime_dir {
    runtime_dir.to_path_buf()
  } else if let Some(runtime_dir) = get_runtime_dir() {
    runtime_dir
  } else {
    // Create the runtime directory if not already present
    let runtime_dir = if !is_portable_mode() {
      get_default_runtime_path()
    } else {
      get_portable_runtime_path().expect("unable to obtain runtime directory path")
    };
    info!("creating runtime directory in {:?}", runtime_dir);
    create_dir_all(&runtime_dir).expect("unable to create runtime directory");
    runtime_dir
  };
}