//! A separate, tightly framed Windows large icon; the title-bar icon is untouched.

use std::io;

const ICO: &[u8] = include_bytes!("../icons/taskbar.ico");
const WM_SETICON: u32 = 0x0080;
const WM_GETICON: u32 = 0x007f;
const ICON_BIG: usize = 1;

#[link(name = "user32")]
extern "system" {
    fn CreateIconFromResourceEx(
        bits: *mut u8,
        size: u32,
        is_icon: i32,
        version: u32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> isize;
    fn SendMessageW(window: isize, message: u32, parameter: usize, value: isize) -> isize;
    fn DestroyIcon(icon: isize) -> i32;
}

/// Owns the HICON until Tauri has destroyed the window and releases app state.
pub struct TaskbarIcon(isize);

impl Drop for TaskbarIcon {
    fn drop(&mut self) {
        unsafe { DestroyIcon(self.0) };
    }
}

fn create() -> Result<TaskbarIcon, String> {
    if ICO.len() < 22 || ICO[..6] != [0, 0, 1, 0, 1, 0] {
        return Err("The embedded taskbar icon has an invalid ICO header.".into());
    }
    let width = if ICO[6] == 0 { 256 } else { i32::from(ICO[6]) };
    let height = if ICO[7] == 0 { 256 } else { i32::from(ICO[7]) };
    let length = u32::from_le_bytes(ICO[14..18].try_into().unwrap());
    let offset = u32::from_le_bytes(ICO[18..22].try_into().unwrap()) as usize;
    let end = offset
        .checked_add(length as usize)
        .ok_or("The embedded taskbar icon is too large.")?;
    let mut bits = ICO
        .get(offset..end)
        .ok_or("The embedded taskbar icon is truncated.")?
        .to_vec();
    // No LR_SHARED: this process owns the returned icon and must destroy it.
    let handle = unsafe {
        CreateIconFromResourceEx(bits.as_mut_ptr(), length, 1, 0x0003_0000, width, height, 0)
    };
    if handle == 0 {
        return Err(format!(
            "Cannot load the taskbar icon: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(TaskbarIcon(handle))
}

pub fn install(window: &tauri::WebviewWindow) -> Result<TaskbarIcon, String> {
    let icon = create()?;
    let handle = window.hwnd().map_err(|error| error.to_string())?;
    // ICON_SMALL is deliberately left as configured by Tauri. Only the large
    // icon used by the running taskbar/Alt+Tab receives the tighter artwork.
    let window = handle.0 as isize;
    unsafe {
        let small_before = SendMessageW(window, WM_GETICON, 0, 0);
        let previous_big = SendMessageW(window, WM_SETICON, ICON_BIG, icon.0);
        if SendMessageW(window, WM_GETICON, ICON_BIG, 0) != icon.0
            || SendMessageW(window, WM_GETICON, 0, 0) != small_before
        {
            SendMessageW(window, WM_SETICON, ICON_BIG, previous_big);
            return Err("Windows could not apply the separate taskbar icon.".into());
        }
    }
    Ok(icon)
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_accepts_the_embedded_taskbar_icon() {
        // Exercise the actual native decoder and ownership path without a window.
        let icon = super::create().expect("The embedded ICO must load on Windows");
        assert_ne!(icon.0, 0);
    }
}
