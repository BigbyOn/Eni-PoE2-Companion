//! Text into the Windows clipboard (regex strings, export paths).
//! From poe2overlay (MIT, © 2026 Michael Malura); see NOTICE.

use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;

pub fn copy(text: &str) -> windows::core::Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        OpenClipboard(None)?;
        let result = (|| {
            EmptyClipboard()?;
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)?;
            std::ptr::copy_nonoverlapping(wide.as_ptr(), GlobalLock(mem) as *mut u16, wide.len());
            let _ = GlobalUnlock(mem);
            // The clipboard owns the memory from here on.
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0)))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Monotonic clipboard change counter: lets a watcher react to Ctrl+C even
/// when the copied item text is identical to the previous one.
pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// Reads Unicode text from the clipboard (used for item import).
pub fn read() -> windows::core::Result<String> {
    unsafe {
        OpenClipboard(None)?;
        let result = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT.0 as u32)?;
            let hglobal = HGLOBAL(handle.0);
            let ptr = GlobalLock(hglobal) as *const u16;
            if ptr.is_null() {
                return Ok(String::new());
            }
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(hglobal);
            Ok(text)
        })();
        let _ = CloseClipboard();
        result
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "touches the real clipboard: cargo test -- --ignored"]
    fn copies_unicode_text() {
        let text = "\"!ab|ä\" mov.*spe";
        super::copy(text).unwrap();
    }
}
