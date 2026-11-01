use anyhow::Result;
use clap::Parser;
use soulframe_audio::cli::Cmd;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(name = "soulframe-audio", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

impl Cli {
    fn into_command(self) -> Cmd {
        self.command.unwrap_or_else(|| {
            Self::parse_from(["soulframe-audio", "extract"])
                .command
                .expect("extract is a valid subcommand")
        })
    }
}

fn main() -> ExitCode {
    let exit = match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    };
    pause_if_owns_console();
    exit
}

fn run() -> Result<()> {
    let command = Cli::parse().into_command();
    let Cmd::Extract {
        output, list_paths, ..
    } = &command;
    if !list_paths {
        println!("Audio output: {}", std::path::absolute(output)?.display());
    }
    soulframe_audio::cli::run(command)
}

#[cfg(windows)]
fn pause_if_owns_console() {
    use std::io::{BufRead, Write};
    use windows_sys::Win32::System::Console::GetConsoleProcessList;

    let mut processes = [0u32; 4];
    // Explorer gives a double-clicked app its own console. Keep the results
    // visible there; a shell invocation returns to the prompt normally.
    if unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) } == 1 {
        let _ = write!(std::io::stdout(), "\nPress Enter to exit...");
        let _ = std::io::stdout().flush();
        let _ = std::io::stdin().lock().read_line(&mut String::new());
    }
}

#[cfg(not(windows))]
fn pause_if_owns_console() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn running_without_arguments_extracts_all_audio_with_the_normal_defaults() {
        for argv in [vec!["soulframe-audio"], vec!["soulframe-audio", "extract"]] {
            let Cmd::Extract {
                cache_path,
                game_exe,
                output,
                path_filter,
                limit,
                list_paths,
                force,
            } = Cli::try_parse_from(argv).unwrap().into_command();
            assert!(cache_path.is_none() && game_exe.is_none() && path_filter.is_none());
            assert_eq!(output, Path::new("output/audio"));
            assert_eq!(limit, 0);
            assert!(!list_paths && !force);
        }
    }
}
