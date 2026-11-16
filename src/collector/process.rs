//! process.rs
//! Win32 네이티브 프로세스 테이블·커맨드라인 조회, 행위자 분류, 로컬 프로세스 종료.

use std::collections::{HashMap, HashSet};
use std::process::Command;
use crate::app::applog;
use crate::app::model::{
    ActorKind, KillError, ProcessRef, ProcessSummary, RiskLevel, StopFailure, StopSuccess,
};
use crate::tui::i18n::{self, Lang};

// 1. 프로세스 레코드 ----------------------------------------------------------
#[derive(Debug, Clone)]
pub struct ProcMetadata {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
    pub actor_kind: ActorKind,
}

#[derive(Debug, Clone)]
pub struct ProcEntry {
    pub name: String,
    pub parent_pid: u32,
}

// 2. Win32 FFI 선언 -----------------------------------------------------------
#[cfg(windows)]
mod ffi {
    use std::ffi::c_void;

    pub type Handle = *mut c_void;

    pub const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    pub const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    pub const PROCESS_TERMINATE: u32 = 0x0001;
    pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    pub const ERROR_ACCESS_DENIED: u32 = 5;
    pub const PROCESS_COMMAND_LINE_INFO: u32 = 60;
    pub const CP_OEMCP: u32 = 1;

    #[repr(C)]
    pub struct ProcessEntry32W {
        pub dw_size: u32,
        pub cnt_usage: u32,
        pub th32_process_id: u32,
        pub th32_default_heap_id: usize,
        pub th32_module_id: u32,
        pub cnt_threads: u32,
        pub th32_parent_process_id: u32,
        pub pc_pri_class_base: i32,
        pub dw_flags: u32,
        pub sz_exe_file: [u16; 260],
    }

    #[repr(C)]
    pub struct UnicodeString {
        pub length: u16,
        pub maximum_length: u16,
        pub buffer: *const u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        pub fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        pub fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        pub fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        pub fn CloseHandle(handle: Handle) -> i32;
        pub fn GetLastError() -> u32;
        pub fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            src: *const u8,
            src_len: i32,
            dst: *mut u16,
            dst_len: i32,
        ) -> i32;
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        pub fn NtQueryInformationProcess(
            handle: Handle,
            info_class: u32,
            info: *mut c_void,
            info_len: u32,
            return_len: *mut u32,
        ) -> i32;
    }
}

// 3. 프로세스 테이블 스냅샷 ---------------------------------------------------
#[cfg(windows)]
pub fn snapshot_process_table() -> HashMap<u32, ProcEntry> {
    let mut table = HashMap::new();
    // SAFETY: 스냅샷 핸들은 이 블록에서만 사용 후 해제, 엔트리는 dw_size 초기화된 POD
    unsafe {
        let snapshot = ffi::CreateToolhelp32Snapshot(ffi::TH32CS_SNAPPROCESS, 0);
        if snapshot.is_null() || snapshot == ffi::INVALID_HANDLE_VALUE {
            return table;
        }
        let mut entry: ffi::ProcessEntry32W = std::mem::zeroed();
        entry.dw_size = std::mem::size_of::<ffi::ProcessEntry32W>() as u32;
        let mut has_entry = ffi::Process32FirstW(snapshot, &mut entry);
        while has_entry != 0 {
            let name_len = entry
                .sz_exe_file
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(entry.sz_exe_file.len());
            table.insert(
                entry.th32_process_id,
                ProcEntry {
                    name: String::from_utf16_lossy(&entry.sz_exe_file[..name_len]),
                    parent_pid: entry.th32_parent_process_id,
                },
            );
            has_entry = ffi::Process32NextW(snapshot, &mut entry);
        }
        ffi::CloseHandle(snapshot);
    }
    table
}

#[cfg(not(windows))]
pub fn snapshot_process_table() -> HashMap<u32, ProcEntry> {
    HashMap::new()
}

// 4. 커맨드라인 조회 ----------------------------------------------------------
#[cfg(windows)]
pub fn query_command_line(pid: u32) -> Option<String> {
    // SAFETY: 응답 UNICODE_STRING 버퍼 포인터가 할당 버퍼 범위 안인지 검증 후 읽음
    unsafe {
        let handle = ffi::OpenProcess(ffi::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }

        // 1. 길이 부족 시 1회 재조회
        let mut words: Vec<u64> = vec![0; 512];
        let mut needed_len = 0u32;
        let mut status = ffi::NtQueryInformationProcess(
            handle,
            ffi::PROCESS_COMMAND_LINE_INFO,
            words.as_mut_ptr().cast(),
            (words.len() * 8) as u32,
            &mut needed_len,
        );
        if status < 0 && needed_len as usize > words.len() * 8 {
            words = vec![0; (needed_len as usize).div_ceil(8)];
            status = ffi::NtQueryInformationProcess(
                handle,
                ffi::PROCESS_COMMAND_LINE_INFO,
                words.as_mut_ptr().cast(),
                (words.len() * 8) as u32,
                &mut needed_len,
            );
        }
        ffi::CloseHandle(handle);
        if status < 0 {
            return None;
        }

        // 2. 버퍼 범위 검증 후 UTF-16 디코딩
        let header = &*(words.as_ptr() as *const ffi::UnicodeString);
        let base_addr = words.as_ptr() as usize;
        let text_addr = header.buffer as usize;
        let byte_len = header.length as usize;
        if header.buffer.is_null()
            || text_addr < base_addr
            || text_addr + byte_len > base_addr + words.len() * 8
        {
            return None;
        }
        let units = std::slice::from_raw_parts(header.buffer, byte_len / 2);
        let text = String::from_utf16_lossy(units).trim().to_string();
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(not(windows))]
pub fn query_command_line(_pid: u32) -> Option<String> {
    None
}

// 5. 행위자 분류 (상위 에이전트 포함) -----------------------------------------
pub fn classify_with_ancestry(
    pid: u32,
    name: &str,
    cmdline: &str,
    table: &HashMap<u32, ProcEntry>,
) -> ActorKind {
    let own_kind = ActorKind::classify(name, cmdline);
    if own_kind == ActorKind::AiAgentMcp {
        return own_kind;
    }

    // 에이전트가 띄운 하위 프로세스 판별 (최대 8단계)
    let mut cursor = table.get(&pid).map(|entry| entry.parent_pid);
    for _ in 0..8 {
        let Some(parent) = cursor.filter(|&ppid| ppid > 4).and_then(|ppid| table.get(&ppid))
        else {
            break;
        };
        let parent_low = parent.name.to_ascii_lowercase();
        if parent_low.contains("claude")
            || parent_low.contains("codex")
            || parent_low.contains("antigravity")
        {
            return ActorKind::AiAgentMcp;
        }
        cursor = Some(parent.parent_pid);
    }
    own_kind
}

// 6. 종료 보호 판별 -----------------------------------------------------------
pub fn is_protected_local_process(pid: u32, proc_name: &str) -> bool {
    if pid <= 4 || pid == std::process::id() {
        return true;
    }
    let lower = proc_name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "system"
            | "idle"
            | "explorer.exe"
            | "windowsterminal.exe"
            | "openconsole.exe"
            | "conhost.exe"
            | "svchost.exe"
            | "csrss.exe"
            | "services.exe"
            | "lsass.exe"
            | "winlogon.exe"
            | "wininit.exe"
            | "database-monitor.exe"
    )
}

pub fn collect_self_ancestor_pids(table: &HashMap<u32, ProcEntry>) -> HashSet<u32> {
    let mut ancestors = HashSet::new();
    let mut cursor = table.get(&std::process::id()).map(|entry| entry.parent_pid);
    while let Some(ppid) = cursor.filter(|&ppid| ppid > 4) {
        if !ancestors.insert(ppid) {
            break;
        }
        cursor = table.get(&ppid).map(|entry| entry.parent_pid);
    }
    ancestors
}

// 7. 로컬 위험 프로세스 강제 종료 (서킷브레이커) ------------------------------
pub fn terminate_local_process(pid: u32, proc_name: &str) -> Result<(), KillError> {
    applog::warn(&format!("Terminating process: {proc_name}({pid})"));
    let result = terminate_checked(pid, proc_name);
    match &result {
        Ok(()) => applog::warn(&format!("Process terminated: {proc_name}({pid})")),
        Err(err) => applog::error(&i18n::kill_error_text(Lang::En, err)),
    }
    result
}

fn terminate_checked(pid: u32, proc_name: &str) -> Result<(), KillError> {
    let process_name = proc_name.to_string();
    if is_protected_local_process(pid, proc_name) {
        return Err(KillError::Protected { process_name, pid });
    }

    // 1. 현재 프로세스 테이블 기준 재확인 (PID 재사용 오인 방지)
    let table = snapshot_process_table();
    let Some(entry) = table.get(&pid) else {
        return Err(KillError::AlreadyExited { process_name, pid });
    };
    if is_protected_local_process(pid, &entry.name) {
        return Err(KillError::Protected {
            process_name: entry.name.clone(),
            pid,
        });
    }
    if proc_name != format!("pid-{pid}") && !entry.name.eq_ignore_ascii_case(proc_name) {
        return Err(KillError::PidReused {
            process_name,
            pid,
            current_name: entry.name.clone(),
        });
    }

    // 2. taskkill /T 자기 종료 방지
    if collect_self_ancestor_pids(&table).contains(&pid) {
        return Err(KillError::MonitorAncestor { process_name, pid });
    }

    // 3. 종료 권한 사전 확인 (권한 없는 taskkill 반복 방지)
    #[cfg(windows)]
    {
        // SAFETY: 확인용 핸들은 열린 직후 해제, GetLastError 는 OpenProcess 직후 호출
        let access_denied = unsafe {
            let handle = ffi::OpenProcess(ffi::PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                ffi::GetLastError() == ffi::ERROR_ACCESS_DENIED
            } else {
                ffi::CloseHandle(handle);
                false
            }
        };
        if access_denied {
            return Err(KillError::AccessDenied {
                process_name,
                pid,
                parent_pid: entry.parent_pid,
            });
        }
    }

    let out = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output()
        .map_err(|e| KillError::SpawnFailed {
            process_name: process_name.clone(),
            pid,
            detail: e.to_string(),
        })?;

    if out.status.success() {
        Ok(())
    } else {
        Err(KillError::TaskkillFailed {
            process_name,
            pid,
            exit_code: out.status.code().unwrap_or(-1),
            detail: decode_console_output(&out.stderr),
        })
    }
}

pub fn terminate_related_local_processes(
    processes: &[ProcessSummary],
) -> Result<StopSuccess, StopFailure> {
    let mut targets: Vec<&ProcessSummary> = Vec::new();
    for p in processes {
        if is_protected_local_process(p.pid, &p.process_name) {
            applog::info(&format!("Bulk stop skipped (protected): {}({})", p.process_name, p.pid));
        } else if p.risk_level < RiskLevel::Critical {
            applog::info(&format!(
                "Bulk stop skipped ({:?}): {}({})",
                p.risk_level, p.process_name, p.pid
            ));
        } else {
            targets.push(p);
        }
    }

    if targets.is_empty() {
        return Err(StopFailure::NoCritical {
            shown: processes.len(),
        });
    }

    let mut stopped = Vec::new();
    let mut failed = Vec::new();

    for proc_item in targets {
        match terminate_local_process(proc_item.pid, &proc_item.process_name) {
            Ok(()) => stopped.push(ProcessRef {
                process_name: proc_item.process_name.clone(),
                pid: proc_item.pid,
            }),
            Err(err) => failed.push(err),
        }
    }

    if stopped.is_empty() {
        return Err(StopFailure::AllFailed { failed });
    }
    if failed.is_empty() {
        Ok(StopSuccess::AllStopped { stopped })
    } else {
        Ok(StopSuccess::Partial { stopped, failed })
    }
}

// taskkill 출력은 OEM 코드페이지(CP949 등)라 UTF-8 아니면 OEM 으로 변환
fn decode_console_output(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.trim().to_string();
    }
    #[cfg(windows)]
    {
        let src_len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
        // SAFETY: 입력 길이는 bytes 범위, 출력 버퍼는 1차 호출이 알려준 길이로 할당
        unsafe {
            let needed = ffi::MultiByteToWideChar(
                ffi::CP_OEMCP,
                0,
                bytes.as_ptr(),
                src_len,
                std::ptr::null_mut(),
                0,
            );
            if needed > 0 {
                let mut units = vec![0u16; needed as usize];
                let written = ffi::MultiByteToWideChar(
                    ffi::CP_OEMCP,
                    0,
                    bytes.as_ptr(),
                    src_len,
                    units.as_mut_ptr(),
                    needed,
                );
                if written > 0 {
                    return String::from_utf16_lossy(&units[..written as usize])
                        .trim()
                        .to_string();
                }
            }
        }
    }
    String::from_utf8_lossy(bytes).trim().to_string()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn native_process_table_and_command_line_include_self() {
        let table = snapshot_process_table();
        let self_entry = table.get(&std::process::id()).expect("self in snapshot");
        assert!(self_entry.name.to_ascii_lowercase().ends_with(".exe"));
        assert!(!collect_self_ancestor_pids(&table).is_empty());

        let cmdline = query_command_line(std::process::id()).expect("self cmdline");
        assert!(cmdline.to_ascii_lowercase().contains("database_monitor"));
    }

    #[test]
    fn child_of_agent_host_is_classified_as_agent() {
        let mut table = HashMap::new();
        table.insert(
            100,
            ProcEntry {
                name: "claude.exe".to_string(),
                parent_pid: 50,
            },
        );
        table.insert(
            200,
            ProcEntry {
                name: "pwsh.exe".to_string(),
                parent_pid: 100,
            },
        );
        table.insert(
            300,
            ProcEntry {
                name: "mysql.exe".to_string(),
                parent_pid: 200,
            },
        );
        table.insert(
            400,
            ProcEntry {
                name: "mysql.exe".to_string(),
                parent_pid: 50,
            },
        );

        assert_eq!(
            classify_with_ancestry(300, "mysql.exe", "mysql.exe -h db", &table),
            ActorKind::AiAgentMcp
        );
        assert_eq!(
            classify_with_ancestry(400, "mysql.exe", "mysql.exe -h db", &table),
            ActorKind::DbClientCli
        );
    }

    #[test]
    fn protected_names_and_self_are_refused() {
        assert!(terminate_local_process(std::process::id(), "x.exe").is_err());
        assert!(terminate_local_process(999_999, "explorer.exe").is_err());
    }
}
