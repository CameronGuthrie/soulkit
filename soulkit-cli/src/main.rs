fn main() -> anyhow::Result<()> {
    let result = soulkit_cli::run_from(std::env::args_os());
    pause_if_owns_console();
    result
}

#[cfg(windows)]
fn pause_if_owns_console() {
    use std::io::IsTerminal;
    use windows_sys::Win32::System::Console::GetConsoleProcessList;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return;
    }
    let mut processes = [0u32; 4];
    let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) };
    if count == 1 {
        use std::io::{BufRead, Write};
        let _ = write!(std::io::stdout(), "\nPress Enter to exit...");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
    }
}

#[cfg(not(windows))]
fn pause_if_owns_console() {}
