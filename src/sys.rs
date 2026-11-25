//! Small Win32 helpers: tray glyph rendering, theme, autostart, tray promotion, DWM corners.

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::SM_CXSMICON;

/// Tray icon edge in pixels for the current system DPI (16 at 100%, 20 at 125%, 24 at 150%).
pub fn tray_size() -> u32 {
    unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()).max(16) as u32 }
}

/// Renders `text` as a crisp, optically centered glyph (RGBA, `size`x`size`).
/// GDI grayscale antialiasing gives coverage; we tint it with `rgb` and scale by `alpha`.
pub fn render_glyph(text: &str, rgb: [u8; 3], alpha: f32, size: u32) -> Vec<u8> {
    let s = size as i32;
    // Supersample 4x, then box-filter down: smoother than GDI AA at 16 px, still pixel-aligned.
    let ss = 4;
    let big = s * ss;
    let mut cov = vec![0u8; (big * big) as usize];
    unsafe {
        let hdc = CreateCompatibleDC(None);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: big,
                biHeight: -big,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(hdc);
            return vec![0; (s * s * 4) as usize];
        };
        let old = SelectObject(hdc, bmp.into());
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, windows::Win32::Foundation::COLORREF(0x00FF_FFFF));
        let wide: Vec<u16> = text.encode_utf16().collect();

        // Largest font whose ink fits the square with a hair of margin.
        let mut h = big * 105 / 100;
        let font = loop {
            let f = CreateFontW(
                -h,
                0,
                0,
                0,
                700,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                NONANTIALIASED_QUALITY,
                0,
                w!("Segoe UI Variable Display"),
            );
            let prev = SelectObject(hdc, f.into());
            let mut sz = Default::default();
            let _ = GetTextExtentPoint32W(hdc, &wide, &mut sz);
            SelectObject(hdc, prev);
            if sz.cx <= big + ss || h <= 8 {
                break f;
            }
            let _ = DeleteObject(f.into());
            h -= ss;
        };
        let prev = SelectObject(hdc, font.into());
        let mut rc = RECT { left: -big, top: 0, right: big * 2, bottom: big };
        let mut wide_mut = wide.clone();
        DrawTextW(hdc, &mut wide_mut, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        let _ = GdiFlush();
        let px = std::slice::from_raw_parts(bits as *const u8, (big * big * 4) as usize);
        for i in 0..(big * big) as usize {
            cov[i] = px[i * 4 + 1];
        }
        SelectObject(hdc, prev);
        let _ = DeleteObject(font.into());
        SelectObject(hdc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(hdc);
    }

    // Optical centering on the ink bounding box (GDI centers the em box, digits sit high).
    let (mut x0, mut y0, mut x1, mut y1) = (big, big, -1, -1);
    for y in 0..big {
        for x in 0..big {
            if cov[(y * big + x) as usize] > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    let (dx, dy) = if x1 < 0 { (0, 0) } else { ((big - 1 - x1 - x0) / 2, (big - 1 - y1 - y0) / 2) };
    // Snap the shift to whole output pixels so stems stay sharp.
    let (dx, dy) = ((dx as f32 / ss as f32).round() as i32 * ss, (dy as f32 / ss as f32).round() as i32 * ss);

    let mut out = vec![0u8; (s * s * 4) as usize];
    for oy in 0..s {
        for ox in 0..s {
            let mut acc = 0u32;
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = ox * ss + sx - dx;
                    let y = oy * ss + sy - dy;
                    if x >= 0 && y >= 0 && x < big && y < big {
                        acc += cov[(y * big + x) as usize] as u32;
                    }
                }
            }
            let a = (acc as f32 / (ss * ss) as f32 * alpha).round().min(255.0) as u8;
            let i = ((oy * s + ox) * 4) as usize;
            out[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
        }
    }
    out
}

fn reg_dword(root: HKEY, key: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut len = 4u32;
    let r = unsafe {
        RegGetValueW(root, key, name, RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as *mut _), Some(&mut len))
    };
    r.is_ok().then_some(data)
}

/// True when the taskbar uses the light theme (glyphs must be dark).
pub fn taskbar_is_light() -> bool {
    reg_dword(
        HKEY_CURRENT_USER,
        w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
        w!("SystemUsesLightTheme"),
    ) == Some(1)
}

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");

pub fn set_autostart(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).is_err() {
            return;
        }
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let v: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
                let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2);
                let _ = RegSetValueExW(key, w!("Pulso"), None, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, w!("Pulso"));
        }
        let _ = RegCloseKey(key);
    }
}

/// Windows 11 parks new tray icons in the overflow menu. Mark Pulso's own entries as promoted
/// (the same flag the "Other system tray icons" toggle writes) so they sit next to the clock.
pub fn promote_tray_icons() {
    let Ok(exe) = std::env::current_exe() else { return };
    let exe = exe.to_string_lossy().to_lowercase();
    unsafe {
        let mut root = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, w!(r"Control Panel\NotifyIconSettings"), None, KEY_READ, &mut root).is_err()
        {
            return;
        }
        let mut i = 0;
        loop {
            let mut name = [0u16; 64];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(root, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None)
                .is_err()
            {
                break;
            }
            i += 1;
            let mut sub = HKEY::default();
            let sub_name = HSTRING::from_wide(&name[..len as usize]);
            if RegOpenKeyExW(root, &sub_name, None, KEY_READ | KEY_SET_VALUE, &mut sub).is_err() {
                continue;
            }
            let mut buf = [0u16; 520];
            let mut blen = (buf.len() * 2) as u32;
            let ok = RegGetValueW(
                sub,
                None,
                w!("ExecutablePath"),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut blen),
            )
            .is_ok();
            if ok {
                let path = String::from_utf16_lossy(&buf[..(blen as usize / 2).saturating_sub(1)]).to_lowercase();
                if path == exe {
                    let one = 1u32.to_le_bytes();
                    let _ = RegSetValueExW(sub, w!("IsPromoted"), None, REG_DWORD, Some(&one));
                }
            }
            let _ = RegCloseKey(sub);
        }
        let _ = RegCloseKey(root);
    }
}

pub fn round_corners(hwnd: isize) {
    unsafe {
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}
