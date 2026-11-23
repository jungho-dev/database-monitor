//! layout_store.rs
//! TUI 상태 파일(tui.toml): 표시 언어·카드 순서·경계 비율·표 열 폭 저장과 복원, 임시 파일 후 이름 바꾸기 방식의 원자적 쓰기.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use serde::{Deserialize, Serialize};
use crate::app::config::TUI_STATE_FILE_NAME;
use crate::tui::cards::{CARD_COUNT, CardId, MAX_COLUMN_WIDTH, MAX_TABLE_COLUMNS};
use crate::tui::i18n::{self, Lang, StateIssue};
use crate::tui::layout::{CardLayout, SplitGroup};
use crate::tui::state::{ActiveTab, AppState, TAB_COUNT, ToastLevel};
use crate::tui::state_writer::StateSave;

const LAYOUT_VERSION: i64 = 1;
const BACKUP_SUFFIX: &str = ".bak";
const TEMP_SUFFIX: &str = ".tmp";
// 심볼릭 링크 사슬 최대 단계 (동기화 폴더로 겹겹이 연결한 구성까지, 순환 링크는 이 단계에서 중단)
const MAX_LINK_DEPTH: usize = 16;
// 이 시간 안에 수정된 임시 파일은 다른 실행 중인 인스턴스가 쓰는 중일 수 있어 지우지 않음
const STALE_TEMP_AGE: Duration = Duration::from_secs(60);

// 1. 파일 형식 ----------------------------------------------------------------
// 언어는 최상위 문자열 (알 수 없는 값만 무시), 그룹·표는 고정 이름 키, 값은 정수 배열 (항목마다 길이·범위를 검사해 맞지 않는 항목만 무시)
#[derive(Debug, Default, Serialize, Deserialize)]
struct LayoutFile {
    version: i64,
    #[serde(default)]
    language: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tab_order: Option<toml::Value>,
    #[serde(default)]
    orders: BTreeMap<String, Vec<i64>>,
    #[serde(default)]
    ratios: BTreeMap<String, Vec<i64>>,
    #[serde(default)]
    column_widths: BTreeMap<String, Vec<i64>>,
}

// 읽기 결과: 파일 없음, 읽은 내용, 읽을 수 없는 파일 (사유)
enum LayoutRead {
    Missing,
    Loaded(LayoutFile),
    Broken(StateIssue),
}

// 2. 현재 배치 -> 파일 내용 ---------------------------------------------------
// 기본 배치와 다른 항목만 기록
fn build_layout_file(
    layout: &CardLayout,
    column_widths: &[[Option<u16>; MAX_TABLE_COLUMNS]; CARD_COUNT],
    lang: Lang,
    tab_order: &[ActiveTab; TAB_COUNT],
) -> LayoutFile {
    let mut file = LayoutFile {
        version: LAYOUT_VERSION,
        language: Some(toml::Value::String(lang.code().to_string())),
        ..LayoutFile::default()
    };
    if *tab_order != ActiveTab::ALL {
        let keys = tab_order
            .iter()
            .map(|tab| toml::Value::String(tab.tab_key().to_string()))
            .collect();
        file.tab_order = Some(toml::Value::Array(keys));
    }
    for group in SplitGroup::ALL {
        if let Some(order) = layout.custom_order(group) {
            let values = order.iter().map(|card| i64::from(*card)).collect();
            file.orders.insert(group.group_key().to_string(), values);
        }
        if let Some(ratios) = layout.custom_ratios(group) {
            let values = ratios.iter().map(|ratio| i64::from(*ratio)).collect();
            file.ratios.insert(group.group_key().to_string(), values);
        }
    }
    for card in CardId::ALL {
        let Some(table) = card.sort_table() else {
            continue;
        };
        let widths = &column_widths[card as usize][..table.column_count()];
        if widths.iter().all(Option::is_none) {
            continue;
        }
        let values = widths
            .iter()
            .map(|width| i64::from(width.unwrap_or(0)))
            .collect();
        file.column_widths
            .insert(table.table_key().to_string(), values);
    }
    file
}

// 3. 파일 내용 -> 현재 배치 ---------------------------------------------------
// 맞지 않는 항목(모르는 이름, 길이·범위 불일치, 열 폭 상한 초과)은 건너뛰고 건너뛴 개수를 반환
fn apply_layout_file(
    file: &LayoutFile,
    layout: &mut CardLayout,
    column_widths: &mut [[Option<u16>; MAX_TABLE_COLUMNS]; CARD_COUNT],
) -> usize {
    let mut skipped = 0usize;
    for (key, values) in &file.orders {
        let group = SplitGroup::ALL
            .iter()
            .find(|group| group.group_key() == key.as_str());
        match group {
            Some(group) if layout.restore_order(*group, values) => {}
            _ => skipped += 1,
        }
    }
    for (key, values) in &file.ratios {
        let group = SplitGroup::ALL
            .iter()
            .find(|group| group.group_key() == key.as_str());
        match group {
            Some(group) if layout.restore_ratios(*group, values) => {}
            _ => skipped += 1,
        }
    }
    for (key, values) in &file.column_widths {
        let card = CardId::ALL.iter().find(|card| {
            card.sort_table()
                .is_some_and(|table| table.table_key() == key.as_str())
        });
        let Some(card) = card else {
            skipped += 1;
            continue;
        };
        let columns = card.sort_table().map_or(0, |table| table.column_count());
        let parsed: Option<Vec<Option<u16>>> = values
            .iter()
            .map(|value| match u16::try_from(*value) {
                Ok(0) => Some(None),
                Ok(width) if width <= MAX_COLUMN_WIDTH => Some(Some(width)),
                _ => None,
            })
            .collect();
        match parsed {
            Some(widths) if widths.len() == columns => {
                let slot = &mut column_widths[*card as usize];
                *slot = [None; MAX_TABLE_COLUMNS];
                slot[..columns].copy_from_slice(&widths);
            }
            _ => skipped += 1,
        }
    }
    skipped
}

// 탭 순서 값 -> 네 탭을 겹침 없이 모두 담은 순서 (배열이 아니거나 모르는 이름·중복·개수 불일치면 None)
fn parse_tab_order(value: &toml::Value) -> Option<[ActiveTab; TAB_COUNT]> {
    let keys = value.as_array()?;
    let mut tabs: Vec<ActiveTab> = Vec::with_capacity(keys.len());
    for key in keys {
        let name = key.as_str()?;
        let tab = ActiveTab::ALL.into_iter().find(|tab| tab.tab_key() == name)?;
        tabs.push(tab);
    }
    let order = <[ActiveTab; TAB_COUNT]>::try_from(tabs).ok()?;
    ActiveTab::ALL
        .iter()
        .all(|tab| order.contains(tab))
        .then_some(order)
}

// 4. 파일 읽기·쓰기 -----------------------------------------------------------
fn read_layout_file(path: &Path) -> LayoutRead {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return LayoutRead::Missing,
        Err(error) => return LayoutRead::Broken(StateIssue::Detail(error.to_string())),
    };
    match toml::from_str::<LayoutFile>(&text) {
        Ok(file) if file.version == LAYOUT_VERSION => LayoutRead::Loaded(file),
        Ok(file) => LayoutRead::Broken(StateIssue::UnsupportedVersion(file.version)),
        Err(error) => LayoutRead::Broken(StateIssue::Detail(error.to_string())),
    }
}

// 읽을 수 없는 원본을 같은 폴더의 tui.toml.bak 으로 복사 (이전 .bak 덮어쓰기, 실패는 무시)
fn backup_state_file(path: &Path) {
    let Some(file_name) = path.file_name() else {
        return;
    };
    let mut backup_name = file_name.to_os_string();
    backup_name.push(BACKUP_SUFFIX);
    let _ = fs::copy(path, path.with_file_name(backup_name));
}

// 심볼릭 링크 사슬을 끝까지 따라가 가리키는 실제 파일 경로로 풀기 (링크가 아니면 그대로, 대상 파일이 아직 없어도 마지막 링크 값까지 따라감)
// 교체는 링크가 아니라 실제 파일 위로 해야 링크가 유지됨, 단계 상한을 넘으면 오류 (순환 링크가 끝없이 돌지 않음)
pub fn resolve_state_path(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_LINK_DEPTH {
        let is_link = fs::symlink_metadata(&current).is_ok_and(|meta| meta.file_type().is_symlink());
        if !is_link {
            return Ok(current);
        }
        let target = fs::read_link(&current)?;
        current = current
            .parent()
            .map_or_else(|| target.clone(), |dir| dir.join(&target));
    }
    Err(io::Error::new(ErrorKind::InvalidInput, "symlink chain too deep"))
}

// 시작할 때 중단된 저장이 남긴 tui.toml.<pid>.tmp 삭제 (파일만, 실패는 무시), 링크 폴더와 실제 파일 폴더 둘 다
fn remove_stale_temp_files(path: &Path) {
    remove_stale_temp_files_in(path);
    if let Ok(target) = resolve_state_path(path)
        && target != path
    {
        remove_stale_temp_files_in(&target);
    }
}

// 한 파일 이름 기준으로 그 폴더의 오래된 임시 파일만 삭제
// 이 프로세스 것이거나 STALE_TEMP_AGE 안에 수정된 임시 파일은 다른 실행 중인 인스턴스가 쓰는 중일 수 있어 남김
fn remove_stale_temp_files_in(path: &Path) {
    let (Some(dir), Some(file_name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let prefix = format!("{}.", file_name.to_string_lossy());
    let own_pid = std::process::id().to_string();
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = name
            .strip_prefix(prefix.as_str())
            .and_then(|rest| rest.strip_suffix(TEMP_SUFFIX))
        else {
            continue;
        };
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        if !is_file || pid.is_empty() || pid == own_pid || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= STALE_TEMP_AGE);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

// 링크를 푼 실제 파일의 폴더에 임시 파일을 쓰고 디스크까지 밀어 낸(sync_all) 뒤 실제 파일 위로 이름 바꿔 교체
// 쓰기 도중 중단돼도 기존 파일은 온전, 링크는 그대로, 실패하면 임시 파일 삭제 (저장 스레드가 호출)
pub(crate) fn write_layout_atomic(path: &Path, text: &str) -> io::Result<()> {
    let target = resolve_state_path(path)?;
    let Some(dir) = target.parent() else {
        return Err(io::Error::new(ErrorKind::InvalidInput, "no parent directory"));
    };
    fs::create_dir_all(dir)?;
    let file_name = target.file_name().map_or_else(
        || TUI_STATE_FILE_NAME.to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let temp_path = dir.join(format!("{file_name}.{}{TEMP_SUFFIX}", std::process::id()));
    let written = fs::File::create(&temp_path).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    });
    if let Err(error) = written {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp_path, &target) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    Ok(())
}

impl AppState {
    // 5. 시작할 때 한 번 복원 -------------------------------------------------
    // 파일이 없으면 영어·기본 배치, 읽을 수 없는 파일이나 맞지 않는 항목(알 수 없는 언어 포함)은 무시하고 알림 한 번
    pub fn load_tui_state(&mut self) {
        let Some(path) = self.state_file.clone() else {
            return;
        };
        remove_stale_temp_files(&path);
        match read_layout_file(&path) {
            LayoutRead::Missing => {}
            LayoutRead::Broken(issue) => {
                // 첫 저장이 덮기 전에 원본 보존
                backup_state_file(&path);
                self.show_toast(
                    &i18n::state_unreadable(self.lang, TUI_STATE_FILE_NAME, &issue),
                    ToastLevel::Info,
                );
            }
            LayoutRead::Loaded(file) => {
                let mut skipped =
                    apply_layout_file(&file, &mut self.card_layout, &mut self.column_widths);
                match file
                    .language
                    .as_ref()
                    .map(|value| value.as_str().and_then(Lang::from_code))
                {
                    None => {}
                    Some(Some(lang)) => self.lang = lang,
                    Some(None) => skipped += 1,
                }
                // 탭 순서는 고정 이름을 겹침 없이 모두 담은 배열만 복원
                match file.tab_order.as_ref().map(parse_tab_order) {
                    None => {}
                    Some(Some(order)) => self.tab_order = order,
                    Some(None) => skipped += 1,
                }
                if skipped > 0 {
                    self.show_toast(
                        &i18n::state_entries_skipped(self.lang, TUI_STATE_FILE_NAME, skipped),
                        ToastLevel::Info,
                    );
                }
            }
        }
    }

    // 6. 확정 때마다 저장 요청 만들기 -----------------------------------------
    // 확정 이벤트 한 번에 파일 내용 전체를 만들어 저장 요청 하나로 돌려줌, 상태 파일 경로가 없으면 None
    // 파일 쓰기는 입력 스레드가 아니라 저장 스레드(state_writer)가 하고 실패는 UiMessage 로 돌아옴
    pub fn build_state_save(&self) -> Result<Option<StateSave>, String> {
        let Some(path) = self.state_file.clone() else {
            return Ok(None);
        };
        let file = build_layout_file(
            &self.card_layout,
            &self.column_widths,
            self.lang,
            &self.tab_order,
        );
        toml::to_string(&file)
            .map(|text| Some(StateSave { path, text }))
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::layout::IDENTITY_ORDER;
    use tempfile::TempDir;

    // 테스트마다 따로 쓰는 임시 폴더의 상태 파일 (실제 홈은 건드리지 않음)
    fn state_in(dir: &TempDir) -> AppState {
        let mut state = AppState::new(100);
        state.state_file = Some(dir.path().join(TUI_STATE_FILE_NAME));
        state.lang = Lang::Ko;
        state
    }

    impl AppState {
        // 저장 스레드 없이 같은 쓰기를 바로 실행하는 테스트 도구 (실패는 오류 알림)
        fn save_tui_state(&mut self) {
            let failure = match self.build_state_save() {
                Ok(Some(save)) => write_layout_atomic(&save.path, &save.text)
                    .err()
                    .map(|error| error.to_string()),
                Ok(None) => None,
                Err(detail) => Some(detail),
            };
            if let Some(detail) = failure {
                self.show_toast(&i18n::state_save_failed(self.lang, &detail), ToastLevel::Error);
            }
        }
    }

    fn customize(state: &mut AppState) {
        state
            .card_layout
            .move_card_to_slot(SplitGroup::MetricCols, &IDENTITY_ORDER, 3, 0);
        state
            .card_layout
            .move_card_to_slot(SplitGroup::AlertsRows, &IDENTITY_ORDER, 0, 1);
        assert!(state.card_layout.restore_ratios(SplitGroup::TopSqlRows, &[6_000]));
        assert!(
            state
                .card_layout
                .restore_ratios(SplitGroup::MetricCols, &[1_000, 3_000, 5_000])
        );
        state.column_widths[CardId::Processes as usize][1] = Some(14);
        state.column_widths[CardId::Processes as usize][3] = Some(9);
        state.column_widths[CardId::TopSqlRanking as usize][8] = Some(21);
    }

    // 1. 저장 후 복원
    #[test]
    fn save_then_load_round_trips_orders_ratios_and_column_widths() {
        let dir = TempDir::new().unwrap();
        let mut saved = state_in(&dir);
        customize(&mut saved);
        saved.save_tui_state();
        assert!(saved.toast.is_none(), "저장 실패 알림: {:?}", saved.toast);

        let mut loaded = state_in(&dir);
        loaded.lang = Lang::En;
        loaded.load_tui_state();
        assert!(loaded.toast.is_none(), "복원 알림: {:?}", loaded.toast);
        assert_eq!(loaded.lang, Lang::Ko);
        assert_eq!(loaded.card_layout, saved.card_layout);
        assert_eq!(loaded.column_widths, saved.column_widths);
        assert_eq!(loaded.card_layout.order_of(SplitGroup::MetricCols), [3, 0, 1, 2]);
        assert_eq!(
            loaded.column_widths[CardId::Processes as usize][..4],
            [None, Some(14), None, Some(9)]
        );
    }

    // 2. 파일 형식
    #[test]
    fn saved_file_text_keys_entries_by_stable_names_with_the_version_first() {
        let dir = TempDir::new().unwrap();
        let mut state = state_in(&dir);
        customize(&mut state);
        state.save_tui_state();
        let text = fs::read_to_string(dir.path().join(TUI_STATE_FILE_NAME)).unwrap();
        let expected = "version = 1\n\
            language = \"ko\"\n\
            \n\
            [orders]\n\
            alerts_rows = [1, 0]\n\
            metric_cols = [3, 0, 1, 2]\n\
            \n\
            [ratios]\n\
            metric_cols = [1000, 3000, 5000]\n\
            top_sql_rows = [6000]\n\
            \n\
            [column_widths]\n\
            processes = [0, 14, 0, 9, 0, 0, 0, 0, 0]\n\
            top_sql = [0, 0, 0, 0, 0, 0, 0, 0, 21]\n";
        assert_eq!(text, expected);
    }

    // 3. 기본 배치 복귀
    #[test]
    fn reset_overwrites_custom_entries_and_keeps_the_language() {
        let dir = TempDir::new().unwrap();
        let mut state = state_in(&dir);
        customize(&mut state);
        state.save_tui_state();

        state.card_layout.reset_groups(&SplitGroup::ALL);
        state.column_widths = [[None; MAX_TABLE_COLUMNS]; CARD_COUNT];
        state.save_tui_state();
        let text = fs::read_to_string(dir.path().join(TUI_STATE_FILE_NAME)).unwrap();
        assert!(text.contains("version = 1"), "{text}");
        assert!(text.contains("language = \"ko\""), "{text}");
        assert!(!text.contains("metric_cols"), "{text}");
        assert!(!text.contains("processes"), "{text}");

        let mut loaded = state_in(&dir);
        loaded.load_tui_state();
        assert!(loaded.toast.is_none());
        assert_eq!(loaded.card_layout, CardLayout::default());
    }

    // 4. 파일 없음
    #[test]
    fn missing_file_keeps_defaults_without_a_toast_or_a_file() {
        let dir = TempDir::new().unwrap();
        let mut state = state_in(&dir);
        state.load_tui_state();
        assert!(state.toast.is_none());
        assert_eq!(state.card_layout, CardLayout::default());
        assert!(!dir.path().join(TUI_STATE_FILE_NAME).exists());
    }

    // 5. 읽을 수 없는 파일 백업
    #[test]
    fn unreadable_or_other_version_file_is_backed_up_once_before_any_overwrite() {
        for body in [
            "이건 toml 이 아님 = = [",
            "version = 2\n[orders]\nmetric_cols = [3, 0, 1, 2]\n",
            "orders = 5\n",
            "version = \"1\"\n",
        ] {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join(TUI_STATE_FILE_NAME);
            fs::write(&path, body).unwrap();
            let mut state = state_in(&dir);
            state.load_tui_state();
            let toast = state.toast.as_ref().unwrap_or_else(|| panic!("알림 없음: {body}"));
            assert!(toast.text.contains("읽기 실패"), "{}", toast.text);
            assert_eq!(toast.level, ToastLevel::Info);
            assert_eq!(state.card_layout, CardLayout::default(), "{body}");

            // 원본은 .bak 에 보존, 이후 저장은 새 내용으로 교체
            let backup = dir.path().join(format!("{TUI_STATE_FILE_NAME}{BACKUP_SUFFIX}"));
            assert_eq!(fs::read_to_string(&backup).unwrap(), body);
            state.save_tui_state();
            assert_ne!(fs::read_to_string(&path).unwrap(), body);
            assert_eq!(fs::read_to_string(&backup).unwrap(), body);
        }
    }

    // 6. 맞지 않는 항목 건너뛰기
    #[test]
    fn mismatched_entries_are_skipped_while_valid_entries_still_apply() {
        let dir = TempDir::new().unwrap();
        let body = "version = 1\n\
            [orders]\n\
            metric_cols = [3, 0, 1, 2]\n\
            alerts_rows = [0, 1, 2]\n\
            top_sql_rows = [1, 1]\n\
            overview_rows = [0, 1, 2]\n\
            unknown_group = [0, 1]\n\
            [ratios]\n\
            processes_rows = [2000, 3000]\n\
            top_sql_rows = [6000]\n\
            [column_widths]\n\
            alerts = [0, 12]\n\
            processes = [0, 0, 0, 0, 0, 0, 0, 20, 0]\n\
            top_sql = [-5, 0, 0, 0, 0, 0, 0, 0, 0]\n\
            sockets = [0, 513, 0, 0, 0, 0]\n\
            overview_chart = [1]\n";
        fs::write(dir.path().join(TUI_STATE_FILE_NAME), body).unwrap();
        let mut state = state_in(&dir);
        state.load_tui_state();

        // 맞는 항목만 반영: 지표 순서, 랭킹 비율, 프로세스 열 폭
        assert_eq!(
            state.card_layout.custom_order(SplitGroup::MetricCols),
            Some(vec![3, 0, 1, 2])
        );
        assert_eq!(
            state.card_layout.custom_ratios(SplitGroup::TopSqlRows),
            Some(vec![6_000])
        );
        assert_eq!(state.column_widths[CardId::Processes as usize][7], Some(20));

        // 길이·범위·중복·모르는 이름·줄 그룹 순서·상한 초과 열 폭은 무시
        assert_eq!(state.card_layout.custom_order(SplitGroup::AlertsRows), None);
        assert_eq!(state.card_layout.custom_order(SplitGroup::TopSqlRows), None);
        assert_eq!(state.card_layout.custom_order(SplitGroup::OverviewRows), None);
        assert_eq!(state.card_layout.custom_ratios(SplitGroup::ProcessesRows), None);
        for card in [CardId::SqlStream, CardId::Sockets, CardId::TopSqlRanking, CardId::AlertLog] {
            assert!(
                state.column_widths[card as usize].iter().all(Option::is_none),
                "{card:?}"
            );
        }

        // 알림은 한 번, 건너뛴 항목 수 표시
        let toast = state.toast.as_ref().unwrap();
        assert!(toast.text.contains("일부 항목 무시 9건"), "{}", toast.text);
    }

    // 7. 알 수 없는 언어
    #[test]
    fn unknown_language_is_skipped_and_keeps_the_default() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(TUI_STATE_FILE_NAME),
            "version = 1\nlanguage = \"fr\"\n",
        )
        .unwrap();
        let mut state = AppState::new(100);
        state.state_file = Some(dir.path().join(TUI_STATE_FILE_NAME));
        state.load_tui_state();
        assert_eq!(state.lang, Lang::En);
        assert!(state.toast.as_ref().unwrap().text.contains("1 entries ignored"));
    }

    // 수정 시각을 지금으로부터 age 전으로 돌린 파일 만들기
    fn write_aged(path: &Path, age: Duration) {
        fs::write(path, "x").unwrap();
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    // 8. 남은 임시 파일 정리
    #[test]
    fn only_stale_temp_files_are_removed_at_startup_and_a_live_instances_temp_file_stays() {
        let dir = TempDir::new().unwrap();
        let stale = dir.path().join(format!("{TUI_STATE_FILE_NAME}.4242{TEMP_SUFFIX}"));
        let fresh = dir.path().join(format!("{TUI_STATE_FILE_NAME}.4343{TEMP_SUFFIX}"));
        let own = dir
            .path()
            .join(format!("{TUI_STATE_FILE_NAME}.{}{TEMP_SUFFIX}", std::process::id()));
        let unrelated = dir.path().join("notes.txt");
        let lookalike = dir.path().join(format!("{TUI_STATE_FILE_NAME}.abc{TEMP_SUFFIX}"));
        write_aged(&stale, STALE_TEMP_AGE + Duration::from_secs(60));
        write_aged(&fresh, Duration::from_secs(1));
        write_aged(&own, STALE_TEMP_AGE + Duration::from_secs(60));
        write_aged(&unrelated, STALE_TEMP_AGE + Duration::from_secs(60));
        write_aged(&lookalike, STALE_TEMP_AGE + Duration::from_secs(60));
        let mut state = state_in(&dir);
        state.load_tui_state();

        // 오래된 임시 파일만 지움: 방금 쓴 다른 인스턴스 것, 자기 자신 것, 이름이 다른 파일은 남김
        assert!(!stale.exists());
        assert!(fresh.exists());
        assert!(own.exists());
        assert!(unrelated.exists());
        assert!(lookalike.exists());
    }

    // 9. 원자적 쓰기
    #[test]
    fn atomic_write_replaces_the_file_and_leaves_no_temp_file() {
        let dir = TempDir::new().unwrap();
        let mut state = state_in(&dir);
        state.save_tui_state();
        customize(&mut state);
        state.save_tui_state();
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [TUI_STATE_FILE_NAME]);
        let text = fs::read_to_string(dir.path().join(TUI_STATE_FILE_NAME)).unwrap();
        assert!(text.contains("metric_cols"), "{text}");
    }

    // 10. 저장 실패
    #[test]
    fn save_failure_becomes_an_error_toast_and_state_without_a_path_is_a_no_op() {
        let dir = TempDir::new().unwrap();
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, "x").unwrap();
        let mut state = AppState::new(100);
        state.state_file = Some(blocker.join(TUI_STATE_FILE_NAME));
        state.save_tui_state();
        assert_eq!(state.toast.as_ref().unwrap().level, ToastLevel::Error);

        let mut homeless = AppState::new(100);
        homeless.save_tui_state();
        homeless.load_tui_state();
        assert!(homeless.toast.is_none());
    }

    // 11. 초기 파일 호환
    #[test]
    fn minimal_initial_file_with_version_and_language_loads_without_a_toast() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(TUI_STATE_FILE_NAME),
            "version = 1
language = \"en\"
",
        )
        .unwrap();
        let mut state = AppState::new(100);
        state.state_file = Some(dir.path().join(TUI_STATE_FILE_NAME));
        state.lang = Lang::Ko;
        state.load_tui_state();
        assert!(state.toast.is_none(), "{:?}", state.toast);
        assert_eq!(state.lang, Lang::En);
        assert_eq!(state.card_layout, CardLayout::default());
    }

    // 16. 탭 순서 저장 왕복
    #[test]
    fn tab_order_round_trips_by_stable_names_and_the_default_order_is_not_written() {
        let dir = TempDir::new().unwrap();
        let mut state = state_in(&dir);
        state.save_tui_state();
        let text = fs::read_to_string(dir.path().join(TUI_STATE_FILE_NAME)).unwrap();
        assert!(!text.contains("tab_order"), "{text}");

        // 바꾼 순서는 최상위 이름 배열로 표보다 앞에 기록
        state.tab_order.swap(0, 1);
        customize(&mut state);
        state.save_tui_state();
        let text = fs::read_to_string(dir.path().join(TUI_STATE_FILE_NAME)).unwrap();
        let expected = "tab_order = [\"processes\", \"overview\", \"top_sql\", \"alerts\"]\n";
        assert!(text.contains(expected), "{text}");
        assert!(text.find("tab_order") < text.find("[orders]"), "{text}");

        let mut loaded = state_in(&dir);
        loaded.load_tui_state();
        assert!(loaded.toast.is_none(), "{:?}", loaded.toast);
        assert_eq!(loaded.tab_order, state.tab_order);
    }

    // 17. 잘못된 탭 순서
    #[test]
    fn broken_tab_order_values_are_skipped_and_the_default_order_stays() {
        for broken in [
            "[\"overview\", \"overview\", \"top_sql\", \"alerts\"]",
            "[\"overview\", \"processes\", \"top_sql\"]",
            "[\"overview\", \"processes\", \"top_sql\", \"alerts\", \"alerts\"]",
            "[\"overview\", \"processes\", \"top_sql\", \"logs\"]",
            "[1, 2, 3, 4]",
            "\"overview\"",
        ] {
            let dir = TempDir::new().unwrap();
            fs::write(
                dir.path().join(TUI_STATE_FILE_NAME),
                format!("version = 1\ntab_order = {broken}\n"),
            )
            .unwrap();
            let mut state = state_in(&dir);
            state.load_tui_state();
            assert_eq!(state.tab_order, ActiveTab::ALL, "{broken}");
            let toast = state
                .toast
                .as_ref()
                .unwrap_or_else(|| panic!("알림 없음: {broken}"));
            assert!(toast.text.contains("일부 항목 무시 1건"), "{}", toast.text);
        }
    }

    // 18. 배치 초기화 뒤 탭 순서 유지
    #[test]
    fn layout_reset_key_keeps_the_tab_order_and_the_saved_file_still_holds_it() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let dir = TempDir::new().unwrap();
        let mut state = state_in(&dir);
        state.tab_order.swap(2, 3);
        let moved = state.tab_order;
        let command = state.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert!(matches!(command, crate::tui::state::UiCommand::SaveState));
        assert_eq!(state.tab_order, moved);
        state.save_tui_state();
        let text = fs::read_to_string(dir.path().join(TUI_STATE_FILE_NAME)).unwrap();
        assert!(
            text.contains("tab_order = [\"overview\", \"processes\", \"alerts\", \"top_sql\"]"),
            "{text}"
        );
    }

    // 링크 생성 (권한이 없으면 사유와 함께 None)
    fn try_symlink(target: &Path, link: &Path) -> Option<()> {
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(target, link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        match made {
            Ok(()) => Some(()),
            Err(error) => {
                eprintln!("심볼릭 링크 테스트 건너뜀: 링크 생성 거부 ({error})");
                None
            }
        }
    }

    // 12. 링크 유지 저장
    #[test]
    fn save_through_a_symlink_keeps_the_link_and_updates_the_target() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        let home = dir.path().join("home");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&home).unwrap();
        let target = repo.join("synced-tui.toml");
        let link = home.join(TUI_STATE_FILE_NAME);
        fs::write(&target, "version = 1
language = \"en\"
").unwrap();
        if try_symlink(&target, &link).is_none() {
            return;
        }

        let mut state = AppState::new(100);
        state.state_file = Some(link.clone());
        state.load_tui_state();
        assert!(state.toast.is_none(), "{:?}", state.toast);
        customize(&mut state);
        state.save_tui_state();
        assert!(state.toast.is_none(), "저장 실패 알림: {:?}", state.toast);

        // 링크는 그대로 같은 대상을 가리키고 대상 파일이 새 내용을 가짐
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), target);
        let text = fs::read_to_string(&target).unwrap();
        assert!(text.contains("metric_cols = [3, 0, 1, 2]"), "{text}");
        assert_eq!(fs::read_to_string(&link).unwrap(), text);

        // 임시 파일은 두 폴더 어디에도 남지 않음
        for folder in [&repo, &home] {
            let leftovers = fs::read_dir(folder)
                .unwrap()
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(TEMP_SUFFIX))
                .count();
            assert_eq!(leftovers, 0, "{folder:?}");
        }

        // 링크를 통해 다시 읽으면 같은 배치
        let mut loaded = AppState::new(100);
        loaded.state_file = Some(link);
        loaded.load_tui_state();
        assert_eq!(loaded.card_layout, state.card_layout);
    }

    // 13. 끊긴 링크 저장
    #[test]
    fn save_through_a_dangling_symlink_creates_the_target_and_keeps_the_link() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        let home = dir.path().join("home");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&home).unwrap();
        let target = repo.join("not-yet.toml");
        let link = home.join(TUI_STATE_FILE_NAME);
        if try_symlink(&target, &link).is_none() {
            return;
        }

        let mut state = AppState::new(100);
        state.state_file = Some(link.clone());
        state.save_tui_state();
        assert!(state.toast.is_none(), "{:?}", state.toast);
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(fs::read_to_string(&target).unwrap().contains("version = 1"));
    }

    // 14. 다단계 링크 사슬 저장
    #[test]
    fn save_through_a_multi_level_symlink_chain_updates_the_final_file_and_keeps_every_link() {
        let dir = TempDir::new().unwrap();
        let [repo, middle, home] = ["repo", "middle", "home"].map(|name| dir.path().join(name));
        for folder in [&repo, &middle, &home] {
            fs::create_dir_all(folder).unwrap();
        }
        let target = repo.join("final.toml");
        let second = middle.join("second.toml");
        let first = home.join(TUI_STATE_FILE_NAME);
        if try_symlink(&target, &second).is_none() || try_symlink(&second, &first).is_none() {
            return;
        }

        // 끝 대상이 아직 없어도 사슬을 끝까지 따라가 실제 파일을 만들고 링크는 그대로
        assert_eq!(resolve_state_path(&first).unwrap(), target);
        let mut state = AppState::new(100);
        state.state_file = Some(first.clone());
        customize(&mut state);
        state.save_tui_state();
        assert!(state.toast.is_none(), "{:?}", state.toast);
        assert!(fs::symlink_metadata(&first).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(&second).unwrap().file_type().is_symlink());
        assert!(fs::read_to_string(&target).unwrap().contains("metric_cols = [3, 0, 1, 2]"));
        for folder in [&repo, &middle, &home] {
            let leftovers = fs::read_dir(folder)
                .unwrap()
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(TEMP_SUFFIX))
                .count();
            assert_eq!(leftovers, 0, "{folder:?}");
        }
    }

    // 15. 순환 링크
    #[test]
    fn a_symlink_loop_stops_at_the_depth_limit_with_an_error_and_replaces_nothing() {
        let dir = TempDir::new().unwrap();
        let first = dir.path().join("loop-a.toml");
        let second = dir.path().join("loop-b.toml");
        if try_symlink(&second, &first).is_none() || try_symlink(&first, &second).is_none() {
            return;
        }
        let error = resolve_state_path(&first).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        assert!(write_layout_atomic(&first, "version = 1\n").is_err());
        assert!(fs::symlink_metadata(&first).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(&second).unwrap().file_type().is_symlink());

        // 읽기 시작도 멈추지 않고 읽을 수 없는 파일로 취급
        let mut state = AppState::new(100);
        state.state_file = Some(first);
        state.load_tui_state();
        assert!(state.toast.is_some());
    }
}
