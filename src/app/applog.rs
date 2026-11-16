//! applog.rs
//! 실행 이력 파일 로그 (시작·종료, 경보, 긴급 중단·프로세스 종료 결과, 패닉).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use chrono::{DateTime, Local};

// 로그 파일 이름 접두어 (뒤에 YYYYMMDD.log)
const LOG_FILE_PREFIX: &str = "database-monitor-";

// 날짜가 바뀌면 같은 폴더의 새 날짜 파일로 넘어감 (자정 넘겨 실행해도 일자별 분리)
struct LogSink {
    dir: PathBuf,
    day: String,
    file: File,
}

static LOG_SINK: Mutex<Option<LogSink>> = Mutex::new(None);

// 1. 로그 파일 열기 -----------------------------------------------------------
pub fn init(dir: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let day = Local::now().format("%Y%m%d").to_string();
    let path = dir.join(format!("{LOG_FILE_PREFIX}{day}.log"));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    if let Ok(mut slot) = LOG_SINK.lock() {
        *slot = Some(LogSink {
            dir: dir.to_path_buf(),
            day,
            file,
        });
    }
    Ok(path)
}

// 2. 한 줄 기록 (미초기화 시 무시) --------------------------------------------
// 새 날짜 파일을 못 열면 기존 파일에 계속 기록
fn write_line(level: &str, message: &str, now: DateTime<Local>) {
    let Ok(mut slot) = LOG_SINK.lock() else {
        return;
    };
    let Some(sink) = slot.as_mut() else {
        return;
    };
    let day = now.format("%Y%m%d").to_string();
    if day != sink.day {
        let path = sink.dir.join(format!("{LOG_FILE_PREFIX}{day}.log"));
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
            sink.file = file;
            sink.day = day;
        }
    }
    let stamp = now.format("%Y-%m-%d %H:%M:%S%.3f");
    let _ = writeln!(sink.file, "{stamp} [{level}] {message}");
}

pub fn info(message: &str) {
    write_line("INFO", message, Local::now());
}

pub fn warn(message: &str) {
    write_line("WARN", message, Local::now());
}

pub fn error(message: &str) {
    write_line("ERROR", message, Local::now());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_daily_file_appends_lines_and_rolls_over_at_midnight() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_dir = dir.path().join(crate::app::config::LOG_DIR_NAME);
        let path = init(&log_dir).expect("init");
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(file_name.starts_with("database-monitor-") && file_name.ends_with(".log"), "{file_name}");
        info("시작");
        warn("경보");
        let tomorrow = Local::now() + chrono::Duration::days(1);
        write_line("INFO", "다음 날", tomorrow);

        let text = fs::read_to_string(&path).expect("read log");
        assert!(text.contains("[INFO] 시작"));
        assert!(text.contains("[WARN] 경보"));
        assert!(!text.contains("다음 날"));
        let next_path = log_dir.join(format!("{LOG_FILE_PREFIX}{}.log", tomorrow.format("%Y%m%d")));
        let next_text = fs::read_to_string(&next_path).expect("read rolled log");
        assert!(next_text.contains("[INFO] 다음 날"));
        *LOG_SINK.lock().unwrap() = None;
    }
}
