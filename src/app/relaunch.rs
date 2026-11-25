//! relaunch.rs
//! 실행 파일 교체 감지와 재실행: 기동 시 exe 경로·빌드 식별자 고정, 디스크 식별자 비교, 같은 인자로 최신 exe 실행.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;
use anyhow::{Context, bail};

// 기동 시점 경로 고정 (Linux 교체 뒤 " (deleted)" 회피)
pub static EXE_PATH: LazyLock<Option<PathBuf>> = LazyLock::new(|| std::env::current_exe().ok());
// 기동 시점 빌드 식별자 (빈 값이면 감지 안 함)
pub static BUILD_ID: LazyLock<String> =
    LazyLock::new(|| EXE_PATH.as_deref().map(read_build_id).unwrap_or_default());

// 1. 교체 판정 ----------------------------------------------------------------
// 같은 새 식별자 두 번 연속일 때만 교체 (링크·서명 중 오판 방지)
pub struct ExeWatch {
    startup: String,
    last: String,
}

impl ExeWatch {
    pub fn new(startup: String) -> Self {
        Self {
            startup,
            last: String::new(),
        }
    }

    pub fn observe(&mut self, disk: &str) -> bool {
        let replaced = !self.startup.is_empty()
            && !disk.is_empty()
            && disk != self.startup
            && disk == self.last;
        self.last = disk.to_string();
        replaced
    }
}

// 2. 빌드 식별자 --------------------------------------------------------------
// main 첫 줄에서 고정
pub fn init_build_identity() {
    LazyLock::force(&EXE_PATH);
    LazyLock::force(&BUILD_ID);
}

// "<파일 크기>-<수정시각 UNIX ms>", 읽기 실패는 빈 문자열 (감지 생략)
pub fn read_build_id(path: &Path) -> String {
    let Ok(metadata) = std::fs::metadata(path) else {
        return String::new();
    };
    let Ok(modified) = metadata.modified() else {
        return String::new();
    };
    let Ok(since_epoch) = modified.duration_since(UNIX_EPOCH) else {
        return String::new();
    };
    format!("{}-{}", metadata.len(), since_epoch.as_millis())
}

// 3. 최신 exe 재실행 ----------------------------------------------------------
// 같은 인자로 실행 후 대기, 반환 = 자식 종료 코드
pub fn rerun_latest_exe() -> anyhow::Result<i32> {
    let Some(path) = EXE_PATH.as_deref() else {
        bail!("current executable path unknown");
    };
    let status = Command::new(path)
        .args(std::env::args_os().skip(1))
        .status()
        .with_context(|| format!("rerun {} failed", path.display()))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. 식별자 형식
    #[test]
    fn build_id_is_the_size_and_modified_millis_of_the_file_and_empty_for_a_missing_path() {
        let path = std::env::current_exe().unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let millis = metadata
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        assert_eq!(read_build_id(&path), format!("{}-{millis}", metadata.len()));
        assert_eq!(read_build_id(Path::new("missing-dir/database-monitor.exe")), "");
    }

    // 2. 교체 판정
    #[test]
    fn exe_watch_reports_a_replacement_only_on_the_second_matching_new_value() {
        let mut watch = ExeWatch::new("100-1".to_string());
        assert!(!watch.observe("100-1"));
        assert!(!watch.observe("200-2"));
        assert!(watch.observe("200-2"));
        assert!(!watch.observe(""));
        assert!(!watch.observe(""));
        let mut unknown = ExeWatch::new(String::new());
        assert!(!unknown.observe("200-2"));
        assert!(!unknown.observe("200-2"));
    }
}
