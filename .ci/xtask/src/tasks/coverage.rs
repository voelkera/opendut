use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tracing::{debug, info, warn};

use crate::core::constants;
use crate::core::dependency::Crate;
use crate::core::util::RunRequiringSuccess;

/// Generate a unit test coverage report
#[derive(Debug, clap::Parser)]
pub struct CoverageCli;

impl CoverageCli {
    pub fn default_handling(self) -> crate::Result {
        coverage()
    }
}


#[tracing::instrument]
pub fn coverage() -> crate::Result {
    crate::util::install_crate(Crate::CargoTarpaulin)?;

    clean()?;

    let out_dir = out_dir();
    fs::create_dir_all(&out_dir)?;

    Command::new("cargo-tarpaulin")
        .args([
            "--verbose",
            "--all-features",
            "--workspace",
            "--timeout=30",
            "--out", "xml", "html", "lcov",
            "--output-dir", out_dir.to_str().unwrap(),
            "--all-targets",
            "--doc"
        ])
        .env("RUSTC_BOOTSTRAP", "1")
        .run_requiring_success()?;

    let files = fs::read_dir(&out_dir)?
        .filter_map(|entry| {
            entry
                .inspect_err(|cause| warn!("Ignoring coverage file which could not be read: {cause}"))
                .ok()
        })
        .filter(|entry| entry.path().is_file());

    for file in files {
        let file_name = file.file_name().into_string().unwrap();
        fs::rename(file.path(), out_dir.join(format!("coverage.{file_name}")))?;
    }

    info!("Placed coverage files into: {}", out_dir.display());

    Ok(())
}

#[tracing::instrument]
pub fn clean() -> crate::Result {
    let out_dir = out_dir();
    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)?;
        debug!("Cleaned distribution directory at: {out_dir:?}");
    }
    Ok(())
}

pub fn out_dir() -> PathBuf {
    constants::target_dir().join("coverage")
}
