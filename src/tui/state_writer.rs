//! state_writer.rs
//! 상태 파일 저장 스레드: 확정 이벤트의 저장 요청을 채널로 받아 최신 하나로 합쳐 쓰고, 종료 때 남은 요청을 제한 시간 안에 쓴 뒤 합류하며, 실패는 UiMessage 로 돌려줌.

use std::io;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use crossbeam_channel::{Receiver, RecvTimeoutError, SendError, Sender, bounded, unbounded};
use crate::tui::layout_store::write_layout_atomic;
use crate::tui::state::UiMessage;

// 종료 때 남은 저장 요청을 기다리는 상한 (디스크가 멈춰도 종료가 끝없이 막히지 않음)
pub const STATE_SAVE_JOIN_LIMIT: Duration = Duration::from_secs(5);

// 저장 요청 한 건 (path = 상태 파일 경로, text = UI 스레드가 만든 파일 내용 전체)
pub struct StateSave {
    pub path: PathBuf,
    pub text: String,
}

// 마무리 결과: 모두 씀, 마지막 쓰기가 실패로 끝남(오류 원문), 제한 시간 안에 끝나지 않음
#[derive(Debug, PartialEq, Eq)]
pub enum SaveFinish {
    Clean,
    Failed(String),
    TimedOut,
}

// 1. 저장 스레드 손잡이 -------------------------------------------------------
// 가드가 소유해 어느 종료 경로든 Drop 에서 남은 요청을 쓰고 스레드를 합류시킴
pub struct StateWriter {
    requests: Option<Sender<StateSave>>,
    worker: Option<JoinHandle<()>>,
    finished: Receiver<Option<String>>,
}

impl StateWriter {
    // 저장 요청 보내기 (저장 스레드가 이미 끝났으면 요청을 그대로 돌려줌)
    pub fn queue_state_save(&self, save: StateSave) -> Result<(), SendError<StateSave>> {
        match &self.requests {
            Some(requests) => requests.send(save),
            None => Err(SendError(save)),
        }
    }

    // 요청 채널을 닫아 이미 보낸 요청을 모두 쓴 뒤 스레드 합류, limit 안에 끝나지 않으면 스레드를 떼어 두고 TimedOut
    // 여러 번 불려도 안전 (두 번째부터는 Clean)
    pub fn finish_state_saves(&mut self, limit: Duration) -> SaveFinish {
        self.requests = None;
        let Some(worker) = self.worker.take() else {
            return SaveFinish::Clean;
        };
        match self.finished.recv_timeout(limit) {
            Ok(unresolved) => {
                let _ = worker.join();
                match unresolved {
                    Some(detail) => SaveFinish::Failed(detail),
                    None => SaveFinish::Clean,
                }
            }
            Err(RecvTimeoutError::Timeout) => SaveFinish::TimedOut,
            Err(RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
                SaveFinish::Clean
            }
        }
    }
}

impl Drop for StateWriter {
    fn drop(&mut self) {
        let _ = self.finish_state_saves(STATE_SAVE_JOIN_LIMIT);
    }
}

// 2. 저장 스레드 시작 ---------------------------------------------------------
// 쓰기 실패는 UiMessage::StateSaveFailed 로 UI 스레드에 알림 (알림 문구는 UI 스레드가 화면 언어로 만듦)
pub fn spawn_state_writer(ui_tx: Sender<UiMessage>) -> io::Result<StateWriter> {
    spawn_state_writer_with(ui_tx, write_layout_atomic)
}

// 쓰기 함수를 받는 시작점 (제한 시간 합류를 멈춘 쓰기로 검증할 수 있음)
pub fn spawn_state_writer_with(
    ui_tx: Sender<UiMessage>,
    write: impl Fn(&Path, &str) -> io::Result<()> + Send + 'static,
) -> io::Result<StateWriter> {
    let (requests, inbox) = unbounded::<StateSave>();
    let (done_tx, finished) = bounded::<Option<String>>(1);
    let worker = thread::Builder::new()
        .name("database-monitor-state-writer".to_string())
        .spawn(move || {
            let unresolved = run_state_writer(&inbox, write, |detail| {
                let _ = ui_tx.send(UiMessage::StateSaveFailed { detail });
            });
            let _ = done_tx.send(unresolved);
        })?;
    Ok(StateWriter {
        requests: Some(requests),
        worker: Some(worker),
        finished,
    })
}

// 3. 저장 반복 ----------------------------------------------------------------
// 요청을 받으면 이미 쌓인 요청까지 모아 같은 경로는 가장 나중 것만 쓰고(처음 나온 순서 유지), 실패는 보고하고 계속,
// 보낸 쪽이 모두 닫히면 남은 요청을 쓰고 끝, 반환 = 마지막 쓰기가 실패로 끝났을 때의 오류 원문
// write·report 를 받아 스레드 없이 합치기·실패 보고를 검증할 수 있음
fn run_state_writer(
    inbox: &Receiver<StateSave>,
    write: impl Fn(&Path, &str) -> io::Result<()>,
    report: impl Fn(String),
) -> Option<String> {
    let mut unresolved: Option<String> = None;
    while let Ok(first) = inbox.recv() {
        // 1. 쌓인 요청 모으기
        let mut pending: Vec<StateSave> = vec![first];
        while let Ok(next) = inbox.try_recv() {
            match pending.iter_mut().find(|queued| queued.path == next.path) {
                Some(queued) => queued.text = next.text,
                None => pending.push(next),
            }
        }

        // 2. 쓰기 (성공하면 앞선 실패는 해소)
        for save in &pending {
            match write(&save.path, &save.text) {
                Ok(()) => unresolved = None,
                Err(error) => {
                    let detail = error.to_string();
                    report(detail.clone());
                    unresolved = Some(detail);
                }
            }
        }
    }
    unresolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use std::io::ErrorKind;
    use std::time::Instant;
    use tempfile::TempDir;

    fn save(path: &Path, text: &str) -> StateSave {
        StateSave {
            path: path.to_path_buf(),
            text: text.to_string(),
        }
    }

    #[test]
    fn a_burst_of_requests_for_one_path_writes_only_the_latest_in_first_seen_order() {
        let (requests, inbox) = unbounded::<StateSave>();
        let first = PathBuf::from("first.toml");
        let second = PathBuf::from("second.toml");
        for (path, text) in [
            (&first, "a"),
            (&first, "b"),
            (&second, "x"),
            (&first, "c"),
            (&second, "y"),
        ] {
            requests.send(save(path, text)).unwrap();
        }
        drop(requests);

        let writes: RefCell<Vec<(PathBuf, String)>> = RefCell::new(Vec::new());
        let reports: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let unresolved = run_state_writer(
            &inbox,
            |path, text| {
                writes
                    .borrow_mut()
                    .push((path.to_path_buf(), text.to_string()));
                Ok(())
            },
            |detail| reports.borrow_mut().push(detail),
        );
        assert_eq!(
            *writes.borrow(),
            [(first, "c".to_string()), (second, "y".to_string())]
        );
        assert!(reports.borrow().is_empty());
        assert_eq!(unresolved, None);
    }

    #[test]
    fn a_failed_write_is_reported_and_later_requests_still_run_and_a_later_success_clears_it() {
        let (requests, inbox) = unbounded::<StateSave>();
        let path = PathBuf::from("state.toml");
        requests.send(save(&path, "one")).unwrap();
        drop(requests);
        let reports: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let unresolved = run_state_writer(
            &inbox,
            |_, _| Err(io::Error::new(ErrorKind::PermissionDenied, "access denied")),
            |detail| reports.borrow_mut().push(detail),
        );
        assert_eq!(*reports.borrow(), ["access denied"]);
        assert_eq!(unresolved.as_deref(), Some("access denied"));

        // 실패 뒤 같은 반복에서 성공하면 남은 실패 없음 (보고는 이미 나감)
        let (requests, inbox) = unbounded::<StateSave>();
        let calls = RefCell::new(0usize);
        let (first_path, second_path) = (PathBuf::from("a"), PathBuf::from("b"));
        requests.send(save(&first_path, "1")).unwrap();
        requests.send(save(&second_path, "2")).unwrap();
        drop(requests);
        let unresolved = run_state_writer(
            &inbox,
            |_, _| {
                *calls.borrow_mut() += 1;
                if *calls.borrow() == 1 {
                    Err(io::Error::other("disk full"))
                } else {
                    Ok(())
                }
            },
            |_| {},
        );
        assert_eq!(*calls.borrow(), 2);
        assert_eq!(unresolved, None);
    }

    #[test]
    fn dropping_the_writer_flushes_queued_requests_before_it_returns() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("tui.toml");
        let (ui_tx, ui_rx) = unbounded::<UiMessage>();
        let writer = spawn_state_writer(ui_tx).unwrap();
        writer.queue_state_save(save(&path, "version = 1\n")).unwrap();
        writer
            .queue_state_save(save(&path, "version = 1\nlanguage = \"ko\"\n"))
            .unwrap();
        drop(writer);

        // 합류가 끝난 뒤라 파일은 마지막 요청 내용이고 임시 파일도 남지 않음
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "version = 1\nlanguage = \"ko\"\n"
        );
        let names: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["tui.toml".to_string()]);
        assert!(ui_rx.try_recv().is_err());
    }

    #[test]
    fn a_failed_save_comes_back_as_a_typed_message_and_the_final_flush_reports_it() {
        let dir = TempDir::new().unwrap();
        // 대상 경로가 폴더라 이름 바꾸기가 실패함
        let path = dir.path().join("tui.toml");
        fs::create_dir_all(&path).unwrap();
        let (ui_tx, ui_rx) = unbounded::<UiMessage>();
        let mut writer = spawn_state_writer(ui_tx).unwrap();
        writer.queue_state_save(save(&path, "version = 1\n")).unwrap();

        let finish = writer.finish_state_saves(STATE_SAVE_JOIN_LIMIT);
        let detail = match ui_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            UiMessage::StateSaveFailed { detail } => detail,
            other => panic!("unexpected message: {other:?}"),
        };
        assert!(!detail.is_empty());
        assert_eq!(finish, SaveFinish::Failed(detail));
        let leftovers: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // 끝난 뒤에는 요청을 그대로 돌려주고, 다시 끝내도 안전
        let lost = writer.queue_state_save(save(&path, "late")).unwrap_err();
        assert_eq!(lost.0.text, "late");
        assert_eq!(writer.finish_state_saves(STATE_SAVE_JOIN_LIMIT), SaveFinish::Clean);
    }

    #[test]
    fn a_stuck_write_ends_the_join_at_the_limit_with_timed_out_instead_of_blocking() {
        let (release_tx, release_rx) = bounded::<()>(1);
        let (ui_tx, _ui_rx) = unbounded::<UiMessage>();
        let mut writer = spawn_state_writer_with(ui_tx, move |_, _| {
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
            Ok(())
        })
        .unwrap();
        writer
            .queue_state_save(save(Path::new("stuck.toml"), "x"))
            .unwrap();

        let started = Instant::now();
        let finish = writer.finish_state_saves(Duration::from_millis(150));
        assert_eq!(finish, SaveFinish::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());

        // 멈춘 쓰기를 풀어 주면 떼어 둔 스레드가 정상 종료 (Drop 은 이미 끝난 손잡이라 즉시 반환)
        release_tx.send(()).unwrap();
        assert_eq!(writer.finish_state_saves(Duration::from_millis(10)), SaveFinish::Clean);
    }
}
