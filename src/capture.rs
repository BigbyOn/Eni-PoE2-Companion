//! Screen capture of the game's client area via GDI `BitBlt`.
//!
//! Raw FFI on purpose: the capture needs exactly four GDI calls, and this keeps
//! the code independent of `windows`-crate signature changes.
//!
//! No game memory is read and nothing is injected: this is the same screenshot
//! the Print Screen key would take, cropped to the game window.

#![cfg(windows)]

use std::ffi::c_void;

type Hdc = *mut c_void;
type Hbitmap = *mut c_void;
type Hgdiobj = *mut c_void;

const SRCCOPY: u32 = 0x00CC_0020;
const CAPTUREBLT: u32 = 0x4000_0000;
const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;

#[repr(C)]
struct BitmapInfoHeader {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[repr(C)]
struct RgbQuad {
    b: u8,
    g: u8,
    r: u8,
    reserved: u8,
}

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [RgbQuad; 1],
}

#[link(name = "user32")]
extern "system" {
    fn GetDC(hwnd: *mut c_void) -> Hdc;
    fn ReleaseDC(hwnd: *mut c_void, hdc: Hdc) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    fn CreateCompatibleBitmap(hdc: Hdc, width: i32, height: i32) -> Hbitmap;
    fn SelectObject(hdc: Hdc, obj: Hgdiobj) -> Hgdiobj;
    fn BitBlt(dst: Hdc, x: i32, y: i32, w: i32, h: i32, src: Hdc, sx: i32, sy: i32, rop: u32) -> i32;
    fn GetDIBits(hdc: Hdc, bmp: Hbitmap, start: u32, lines: u32, bits: *mut c_void, info: *mut BitmapInfo, usage: u32) -> i32;
    fn DeleteObject(obj: Hgdiobj) -> i32;
    fn DeleteDC(hdc: Hdc) -> i32;
}

/// Captures a screen rectangle into top-down BGRA pixels.
pub fn capture_rect(left: i32, top: i32, width: i32, height: i32) -> Result<Vec<u8>, String> {
    if width <= 0 || height <= 0 || width > 16_384 || height > 16_384 {
        return Err(format!("Некорректный размер окна игры: {width}×{height}"));
    }
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("Не удалось получить контекст экрана".into());
        }
        let mem = CreateCompatibleDC(screen);
        if mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("Не удалось создать контекст захвата".into());
        }
        let bmp = CreateCompatibleBitmap(screen, width, height);
        if bmp.is_null() {
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("Не удалось создать буфер изображения".into());
        }
        let old = SelectObject(mem, bmp);
        let copied = BitBlt(mem, 0, 0, width, height, screen, left, top, SRCCOPY | CAPTUREBLT);
        SelectObject(mem, old);

        let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
        let mut info = BitmapInfo {
            header: BitmapInfoHeader {
                bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                bi_width: width,
                // Negative height asks for a top-down bitmap.
                bi_height: -height,
                bi_planes: 1,
                bi_bit_count: 32,
                bi_compression: BI_RGB,
                bi_size_image: 0,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            colors: [RgbQuad { b: 0, g: 0, r: 0, reserved: 0 }],
        };
        let lines = GetDIBits(mem, bmp, 0, height as u32, out.as_mut_ptr() as *mut c_void, &mut info, DIB_RGB_COLORS);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);

        if copied == 0 {
            return Err("BitBlt не смог скопировать окно игры в буфер".into());
        }
        if lines == 0 {
            return Err("GetDIBits не вернул пиксели".into());
        }
        Ok(out)
    }
}

/// Swaps the blue and red channel of a 32-bit BGRA buffer in place (RGBA output).
pub fn bgra_to_rgba(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}
