//! A private subprocess entry point for the extraction code linked into Soulkit.

use std::ffi::OsString;
use std::io::Write;

pub const WORKER_FLAG: &str = "--soulkit-worker";

fn extraction_args(args: impl IntoIterator<Item = OsString>) -> Option<Vec<OsString>> {
    let mut args = args.into_iter();
    args.next()?;
    if args.next()? != WORKER_FLAG {
        return None;
    }
    Some(
        std::iter::once(OsString::from("soulkit-cli"))
            .chain(args)
            .collect(),
    )
}

/// Returns None for an ordinary GUI launch. A worker never starts Tauri.
pub fn run_if_requested(args: impl IntoIterator<Item = OsString>) -> Option<i32> {
    let args = extraction_args(args)?;
    let code = if args.len() == 1 {
        eprintln!("An extraction worker requires a command.");
        2
    } else if let Err(error) = crate::runtime_libraries::initialize_worker() {
        eprintln!("Extraction failed: {error}");
        1
    } else {
        let result = if args
            .get(1)
            .is_some_and(|arg| arg == crate::cache_browser::WORKER_COMMAND)
        {
            if args.len() != 3 {
                Err(
                    "The cache extraction worker requires exactly one selection manifest."
                        .to_string(),
                )
            } else {
                crate::cache_browser::run_manifest(std::path::Path::new(&args[2]))
            }
        } else {
            soulkit_cli::run_from(args).map_err(|error| format!("{error:#}"))
        };
        match result {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("Extraction failed: {error:#}");
                1
            }
        }
    };
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_first_argument_enters_the_worker() {
        for args in [
            vec!["soulkit.exe"],
            vec!["soulkit.exe", "--soulkit-worker-extra", "zenith-calendar"],
            vec!["soulkit.exe", "zenith-calendar", "--soulkit-worker"],
        ] {
            assert!(extraction_args(args.into_iter().map(OsString::from)).is_none());
        }
        let args = extraction_args(
            [
                "soulkit.exe",
                WORKER_FLAG,
                "zenith-calendar",
                "--output",
                "a folder/calendar.json",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(
            args,
            [
                "soulkit-cli",
                "zenith-calendar",
                "--output",
                "a folder/calendar.json"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn an_empty_worker_does_not_start_the_default_full_pipeline() {
        assert_eq!(
            run_if_requested(["soulkit.exe", WORKER_FLAG].map(OsString::from)),
            Some(2)
        );
    }
}
