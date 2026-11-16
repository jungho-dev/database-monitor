//! win_ctrl.rs
//! 위험 감지 시 터미널 창 최상단 팝업(HWND_TOPMOST)·작업표시줄 점멸, 클래식 콘솔 글꼴 적용·복원.

#[cfg(windows)]
use std::sync::OnceLock;

// 1. Win32 FFI 선언 -----------------------------------------------------------
#[cfg(windows)]
mod ffi {
    use std::ffi::c_void;

    pub type Hwnd = *mut c_void;
    pub type Handle = *mut c_void;

    pub const HWND_TOPMOST: Hwnd = -1isize as Hwnd;
    pub const HWND_NOTOPMOST: Hwnd = -2isize as Hwnd;
    pub const SWP_NOSIZE: u32 = 0x0001;
    pub const SWP_NOMOVE: u32 = 0x0002;
    pub const SWP_SHOWWINDOW: u32 = 0x0040;
    pub const SW_RESTORE: i32 = 9;
    pub const GA_ROOT: u32 = 2;
    pub const FLASHW_ALL: u32 = 0x0000_0003;
    pub const FLASHW_TIMERNOFG: u32 = 0x0000_000C;
    pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    pub const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    // FF_MODERN(고정폭 계열) | TMPF_TRUETYPE, 굵기 FW_NORMAL
    pub const FONT_FAMILY_TRUETYPE_MODERN: u32 = 0x30 | 0x04;
    pub const FW_NORMAL: u32 = 400;

    #[repr(C)]
    pub struct FlashWInfo {
        pub cb_size: u32,
        pub hwnd: Hwnd,
        pub dw_flags: u32,
        pub u_count: u32,
        pub dw_timeout: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Coord {
        pub x: i16,
        pub y: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct ConsoleFontInfoEx {
        pub cb_size: u32,
        pub n_font: u32,
        pub dw_font_size: Coord,
        pub font_family: u32,
        pub font_weight: u32,
        pub face_name: [u16; 32],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetConsoleWindow() -> Hwnd;
        pub fn GetStdHandle(std_handle: u32) -> Handle;
        pub fn GetCurrentConsoleFontEx(
            output: Handle,
            maximum_window: i32,
            font: *mut ConsoleFontInfoEx,
        ) -> i32;
        pub fn SetCurrentConsoleFontEx(
            output: Handle,
            maximum_window: i32,
            font: *const ConsoleFontInfoEx,
        ) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn GetForegroundWindow() -> Hwnd;
        pub fn GetAncestor(hwnd: Hwnd, ga_flags: u32) -> Hwnd;
        pub fn GetClassNameW(hwnd: Hwnd, class_name: *mut u16, max_count: i32) -> i32;
        pub fn IsIconic(hwnd: Hwnd) -> i32;
        pub fn ShowWindow(hwnd: Hwnd, cmd_show: i32) -> i32;
        pub fn SetWindowPos(
            hwnd: Hwnd,
            hwnd_insert_after: Hwnd,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        pub fn SetForegroundWindow(hwnd: Hwnd) -> i32;
        pub fn BringWindowToTop(hwnd: Hwnd) -> i32;
        pub fn FlashWindowEx(pfwi: *const FlashWInfo) -> i32;
        pub fn keybd_event(b_vk: u8, b_scan: u8, dw_flags: u32, dw_extra_info: usize);
    }
}

#[cfg(windows)]
static INITIAL_TERMINAL_HWND: OnceLock<usize> = OnceLock::new();

// 2. 터미널 창 제어기 ---------------------------------------------------------
pub fn capture_initial_terminal_window() {
    #[cfg(windows)]
    {
        let _ = INITIAL_TERMINAL_HWND.get_or_init(|| unsafe {
            let fg = ffi::GetForegroundWindow();
            if !fg.is_null() {
                let root = ffi::GetAncestor(fg, ffi::GA_ROOT);
                if !root.is_null() {
                    return root as usize;
                }
                return fg as usize;
            }
            let con = ffi::GetConsoleWindow();
            con as usize
        });
    }
}

pub fn elevate_and_flash_terminal(flash: bool) {
    #[cfg(not(windows))]
    let _ = flash;
    #[cfg(windows)]
    unsafe {
        for hwnd in candidate_windows() {
            if hwnd.is_null() {
                continue;
            }
            if ffi::IsIconic(hwnd) != 0 {
                let _ = ffi::ShowWindow(hwnd, ffi::SW_RESTORE);
            }
            let _ = ffi::SetWindowPos(
                hwnd,
                ffi::HWND_TOPMOST,
                0,
                0,
                0,
                0,
                ffi::SWP_NOMOVE | ffi::SWP_NOSIZE | ffi::SWP_SHOWWINDOW,
            );
            // 포그라운드 전환 제약 우회용 더미 키 이벤트
            ffi::keybd_event(0, 0, 0, 0);
            let _ = ffi::BringWindowToTop(hwnd);
            let _ = ffi::SetForegroundWindow(hwnd);

            // 점멸 OFF면 최상단 팝업만 유지
            if !flash {
                continue;
            }
            let info = ffi::FlashWInfo {
                cb_size: std::mem::size_of::<ffi::FlashWInfo>() as u32,
                hwnd,
                dw_flags: ffi::FLASHW_ALL | ffi::FLASHW_TIMERNOFG,
                u_count: 6,
                dw_timeout: 0,
            };
            let _ = ffi::FlashWindowEx(&info);
        }
    }
}

pub fn set_terminal_topmost(enabled: bool) {
    #[cfg(not(windows))]
    let _ = enabled;
    #[cfg(windows)]
    unsafe {
        let mode = if enabled {
            ffi::HWND_TOPMOST
        } else {
            ffi::HWND_NOTOPMOST
        };
        for hwnd in candidate_windows() {
            if !hwnd.is_null() {
                let _ = ffi::SetWindowPos(
                    hwnd,
                    mode,
                    0,
                    0,
                    0,
                    0,
                    ffi::SWP_NOMOVE | ffi::SWP_NOSIZE | ffi::SWP_SHOWWINDOW,
                );
            }
        }
    }
}

#[cfg(windows)]
unsafe fn candidate_windows() -> [ffi::Hwnd; 2] {
    let init_hwnd = INITIAL_TERMINAL_HWND
        .get()
        .copied()
        .unwrap_or(0) as ffi::Hwnd;
    let con_hwnd = unsafe { ffi::GetConsoleWindow() };
    [init_hwnd, con_hwnd]
}

// 3. 클래식 콘솔 글꼴 적용·복원 -----------------------------------------------
// conhost 창에서만 적용, Windows Terminal·VS Code 등 ConPTY 호스트는 각자 프로필 글꼴을 씀 (앱이 못 바꿈)
// 가드가 사라질 때(TUI 종료) 실행 전 글꼴로 되돌림
pub struct ConsoleFontGuard {
    #[cfg(windows)]
    previous: ffi::ConsoleFontInfoEx,
}

impl Drop for ConsoleFontGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: previous 는 GetCurrentConsoleFontEx 가 채운 cb_size 초기화 구조체
        unsafe {
            let output = ffi::GetStdHandle(ffi::STD_OUTPUT_HANDLE);
            if !output.is_null() && output != ffi::INVALID_HANDLE_VALUE {
                let _ = ffi::SetCurrentConsoleFontEx(output, 0, &self.previous);
            }
        }
    }
}

// 적용 성공 시 복원 가드, 적용하지 않은 이유는 Err 문구로 반환 (실행은 계속)
pub fn apply_console_font(face: &str) -> Result<ConsoleFontGuard, String> {
    if face.is_empty() {
        return Err("Font change disabled by configuration (console_font = \"\")".to_string());
    }
    #[cfg(windows)]
    {
        let face_units: Vec<u16> = face.encode_utf16().collect();
        if face_units.len() >= 32 {
            return Err(format!("Font name longer than 31 characters: {face}"));
        }

        // SAFETY: 구조체는 cb_size 를 채운 POD, 출력 핸들·창 핸들은 호출 직전 확인
        unsafe {
            // 0. 실제 conhost 창만 대상 (ConPTY 의사 창은 PseudoConsoleWindow)
            // WT_SESSION 환경 변수는 WT 에서 띄운 conhost 창에도 상속되므로 창 클래스로 판별
            let console_hwnd = ffi::GetConsoleWindow();
            let mut class_buf = [0u16; 64];
            let class_len = if console_hwnd.is_null() {
                0
            } else {
                ffi::GetClassNameW(console_hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32)
            };
            let class_name = String::from_utf16_lossy(&class_buf[..class_len.max(0) as usize]);
            if class_name != "ConsoleWindowClass" {
                return Err(format!(
                    "ConPTY terminal such as Windows Terminal ({class_name}): terminal profile font in use"
                ));
            }

            let output = ffi::GetStdHandle(ffi::STD_OUTPUT_HANDLE);
            if output.is_null() || output == ffi::INVALID_HANDLE_VALUE {
                return Err("No console output handle".to_string());
            }

            // 1. 현재 글꼴 보관
            let mut previous: ffi::ConsoleFontInfoEx = std::mem::zeroed();
            previous.cb_size = std::mem::size_of::<ffi::ConsoleFontInfoEx>() as u32;
            if ffi::GetCurrentConsoleFontEx(output, 0, &mut previous) == 0 {
                return Err("Current console font query failed (not a console window)".to_string());
            }

            // 2. 같은 높이로 TrueType 글꼴 지정 (래스터 글꼴 높이 0 이면 16)
            let mut wanted = previous;
            wanted.n_font = 0;
            wanted.dw_font_size = ffi::Coord {
                x: 0,
                y: if previous.dw_font_size.y > 0 { previous.dw_font_size.y } else { 16 },
            };
            wanted.font_family = ffi::FONT_FAMILY_TRUETYPE_MODERN;
            wanted.font_weight = ffi::FW_NORMAL;
            wanted.face_name = [0; 32];
            wanted.face_name[..face_units.len()].copy_from_slice(&face_units);
            if ffi::SetCurrentConsoleFontEx(output, 0, &wanted) == 0 {
                return Err(format!("Console font change failed: {face}"));
            }

            // 3. 미설치 글꼴은 콘솔이 다른 글꼴로 대체하므로 재조회로 확인, 불일치면 원복
            let mut applied: ffi::ConsoleFontInfoEx = std::mem::zeroed();
            applied.cb_size = previous.cb_size;
            let _ = ffi::GetCurrentConsoleFontEx(output, 0, &mut applied);
            let applied_len = applied.face_name.iter().position(|&unit| unit == 0).unwrap_or(32);
            let applied_face = String::from_utf16_lossy(&applied.face_name[..applied_len]);
            if !applied_face.eq_ignore_ascii_case(face) {
                let _ = ffi::SetCurrentConsoleFontEx(output, 0, &previous);
                return Err(format!("Font not installed or rejected by the console: {face} (now {applied_face})"));
            }
            Ok(ConsoleFontGuard { previous })
        }
    }

    #[cfg(not(windows))]
    {
        Err("Windows-only feature".to_string())
    }
}
