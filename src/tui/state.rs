//! state.rs
//! TUI 화면 상태: 탭, 카드 포커스·배치·스크롤·선택, 수집 스냅샷, 알림, 키 입력 상태 전이.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::app::model::{KillError, MonitorSnapshot, ProcessSummary, RiskLevel, StopReport};
use crate::tui::cards::{
    CARD_COUNT, CardId, CardKind, CardMove, CardPress, CardView, ColumnDrag, CursorMotion,
    MAX_TABLE_COLUMNS, ScrollDrag, UiAction, UiHitAreas,
};
use crate::tui::i18n::{self, Lang, Msg};
use crate::tui::layout::{CardLayout, SplitDrag, SplitGroup};
use crate::tui::sort::{self, SortChange, TableSorts};
use crate::tui::tab_move::TabDrag;
use crate::tui::views::{MIN_HEIGHT, MIN_WIDTH};

pub const TAB_COUNT: usize = 4;
pub const TOAST_LIFETIME: Duration = Duration::from_secs(4);
// 경보 점멸 주기, 위험 지속 시 창 최상단 재호출 간격, 폴링 주기 조절 단위·범위
pub const BLINK_GAP: Duration = Duration::from_millis(260);
const ELEVATE_REPEAT_GAP: Duration = Duration::from_secs(6);
const POLL_STEP_MS: u64 = 250;
const POLL_MIN_MS: u64 = 250;
const POLL_MAX_MS: u64 = 5000;
// 선택 행이 정렬·스냅샷 교체 뒤에도 같은 항목을 따라가는 카드
const SELECTABLE_CARDS: [CardId; 3] = [CardId::Processes, CardId::TopSqlRanking, CardId::AlertLog];

// 1. 탭 -----------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Overview,
    Processes,
    TopSql,
    Alerts,
}

impl ActiveTab {
    pub const ALL: [Self; TAB_COUNT] = [Self::Overview, Self::Processes, Self::TopSql, Self::Alerts];

    pub fn tab_label(self, lang: Lang) -> &'static str {
        let msg = match self {
            Self::Overview => Msg::TabOverview,
            Self::Processes => Msg::TabProcesses,
            Self::TopSql => Msg::TabTopSql,
            Self::Alerts => Msg::TabAlerts,
        };
        i18n::text(lang, msg)
    }

    // 상태 파일의 탭 순서 값 (표시 언어와 무관한 고정 이름)
    pub fn tab_key(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Processes => "processes",
            Self::TopSql => "top_sql",
            Self::Alerts => "alerts",
        }
    }

    // 클릭한 카드가 없을 때 키 입력을 받는 탭의 주 카드
    pub fn main_card(self) -> CardId {
        match self {
            Self::Overview | Self::Processes => CardId::Processes,
            Self::TopSql => CardId::TopSqlRanking,
            Self::Alerts => CardId::AlertLog,
        }
    }

    // 열 폭을 끌어 조절하는 표 카드 (배치 초기화 대상)
    pub fn table_cards(self) -> &'static [CardId] {
        match self {
            Self::Overview => &[CardId::Processes, CardId::Sockets, CardId::SqlStream],
            Self::Processes => &[CardId::Processes],
            Self::TopSql => &[CardId::TopSqlRanking],
            Self::Alerts => &[CardId::AlertLog],
        }
    }

    // 탭 안 카드 경계 그룹 (배치 초기화 대상, 지표 카드 묶음은 모든 탭이 공유)
    pub fn split_groups(self) -> &'static [SplitGroup] {
        match self {
            Self::Overview => &[
                SplitGroup::MetricRows,
                SplitGroup::MetricCols,
                SplitGroup::OverviewRows,
                SplitGroup::OverviewCompactRows,
                SplitGroup::OverviewCols,
            ],
            Self::Processes => &[
                SplitGroup::MetricRows,
                SplitGroup::MetricCols,
                SplitGroup::ProcessesRows,
            ],
            Self::TopSql => &[
                SplitGroup::MetricRows,
                SplitGroup::MetricCols,
                SplitGroup::TopSqlRows,
            ],
            Self::Alerts => &[
                SplitGroup::MetricRows,
                SplitGroup::MetricCols,
                SplitGroup::AlertsRows,
            ],
        }
    }
}

// 2. 알림 ---------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub level: ToastLevel,
    pub expires_at: Instant,
}

// 3. 배경 스레드 → UI 메시지 --------------------------------------------------
// 결과 문구는 화면 언어로 UI 스레드가 만들고 OS·수집기 상세 문자열만 그대로 전달
#[derive(Debug)]
pub enum UiMessage {
    Snapshot(Box<MonitorSnapshot>),
    StopDone(StopReport),
    KillDone {
        pid: u32,
        name: String,
        result: Result<(), KillError>,
    },
    // 상태 파일 저장 스레드의 실패 (detail = 번역하지 않는 오류 원문, 알림 문구는 UI 스레드가 만듦)
    StateSaveFailed {
        detail: String,
    },
    // 디스크 exe 새 빌드 교체 (한 번만)
    ExeReplaced,
}

// 종료 확인 대상: 선택 프로세스 하나, 긴급 중단 대상(위험 등급 프로세스) 목록
#[derive(Debug, Clone)]
pub enum KillConfirm {
    Selected(Box<ProcessSummary>),
    Emergency(Vec<ProcessSummary>),
}

// 4. 입력 → 실행 요청 ---------------------------------------------------------
#[derive(Debug, Clone)]
pub enum UiCommand {
    None,
    RefreshNow,
    SetInterval(u64),
    SetPaused(bool),
    EmergencyStop(Vec<ProcessSummary>),
    KillProcess { pid: u32, name: String },
    SetTopmost(bool),
    // 위험 감지 시 창을 앞으로 올리고 작업표시줄 점멸
    Elevate,
    // 카드 배치(순서·경계·열 폭)나 표시 언어가 확정돼 상태 파일 저장 필요
    SaveState,
}

// 5. 화면 상태 ----------------------------------------------------------------
pub struct AppState {
    // 상태 파일(tui.toml) 경로, 홈이 없으면 저장하지 않음
    pub state_file: Option<PathBuf>,
    pub lang: Lang,
    // 이벤트 루프가 갱신하는 현재 시각 (알림 만료·갱신 신선도 기준, 테스트가 주입)
    pub now: Instant,
    pub tab: ActiveTab,
    // 탭 바 표시 순서 (번호 키·Tab 순환이 따름, tui.toml 로 저장·복원), 탭을 눌러 끄는 중
    pub tab_order: [ActiveTab; TAB_COUNT],
    pub tab_drag: Option<TabDrag>,
    pub snapshot: Option<Arc<MonitorSnapshot>>,
    pub snapshot_at: Option<Instant>,
    pub paused: bool,
    pub poll_interval_ms: u64,
    pub blink_on: bool,
    pub blink_enabled: bool,
    pub last_blink: Instant,
    pub always_on_top: bool,
    pub prev_risk: RiskLevel,
    pub last_elevate: Option<Instant>,
    pub kill_confirm: Option<KillConfirm>,
    // 직전 그리기 크기가 최소 크기 미달이면 파괴적 키·확인이 동작하지 않음 (그리기와 크기 변경 이벤트가 갱신)
    pub too_small: bool,
    // 표별 정렬 (None 이면 수집기 기본 순위)
    pub sorts: TableSorts,
    // 카드별 스크롤·선택 (탭을 오가도 유지)
    pub card_views: [CardView; CARD_COUNT],
    // 카드 경계 비율·순서 (tui.toml 로 저장·복원), 드래그 중·마우스 아래 경계
    pub card_layout: CardLayout,
    pub split_drag: Option<SplitDrag>,
    // 카드 제목을 누른 채 대기 중, 길게 눌러 진입한 카드 이동 중
    pub card_press: Option<CardPress>,
    pub card_move: Option<CardMove>,
    pub split_hover: Option<(SplitGroup, usize)>,
    // 카드별 끌어 정한 열 폭 (None 이면 자동 폭, tui.toml 로 저장·복원), 열 경계 드래그 중·마우스 아래 경계
    // last_column_click = 끌지 않고 뗀 직전 경계 클릭 (더블클릭 판정용)
    pub column_widths: [[Option<u16>; MAX_TABLE_COLUMNS]; CARD_COUNT],
    pub column_drag: Option<ColumnDrag>,
    pub column_hover: Option<(CardId, usize)>,
    pub last_column_click: Option<(CardId, usize, Instant)>,
    // 스크롤바 썸 드래그 중, 마우스가 트랙 위에 있는 카드
    pub scroll_drag: Option<ScrollDrag>,
    pub scroll_hover: Option<CardId>,
    // 클릭한 카드(선택 당시 탭에서만 유효), 마우스 아래 카드
    pub focused_card: Option<(ActiveTab, CardId)>,
    pub hover_card: Option<CardId>,
    pub hit_areas: UiHitAreas,
    pub show_help: bool,
    pub toast: Option<Toast>,
    pub should_quit: bool,
    // 디스크 exe 새 빌드 교체 (강제 재시작 칩 표시), 강제 재시작으로 종료
    pub exe_replaced: bool,
    pub restart_requested: bool,
}

impl AppState {
    // 5-1. 초기 상태
    pub fn new(poll_interval_ms: u64) -> Self {
        let now = Instant::now();
        Self {
            state_file: None,
            lang: Lang::default(),
            now,
            tab: ActiveTab::Overview,
            tab_order: ActiveTab::ALL,
            tab_drag: None,
            snapshot: None,
            snapshot_at: None,
            paused: false,
            poll_interval_ms,
            blink_on: false,
            blink_enabled: true,
            last_blink: now,
            always_on_top: false,
            prev_risk: RiskLevel::Safe,
            last_elevate: None,
            kill_confirm: None,
            too_small: false,
            sorts: TableSorts::default(),
            card_views: [CardView::default(); CARD_COUNT],
            card_layout: CardLayout::default(),
            split_drag: None,
            card_press: None,
            card_move: None,
            split_hover: None,
            column_widths: [[None; MAX_TABLE_COLUMNS]; CARD_COUNT],
            column_drag: None,
            column_hover: None,
            last_column_click: None,
            scroll_drag: None,
            scroll_hover: None,
            focused_card: None,
            hover_card: None,
            hit_areas: UiHitAreas::default(),
            show_help: false,
            toast: None,
            should_quit: false,
            exe_replaced: false,
            restart_requested: false,
        }
    }

    // 5-2. 탭 전환 (카드별 선택·스크롤은 유지, 끌기·강조는 해제)
    // 이미 움직인 경계·열 폭 끌기는 탭을 바꿔도 확정해 저장 (창 밖에서 놓아 놓친 끌기도 포함)
    pub fn switch_tab(&mut self, tab: ActiveTab) -> UiCommand {
        self.cancel_card_move();
        let drag_moved = self.end_drags(None);
        self.last_column_click = None;
        self.tab = tab;
        self.hover_card = None;
        self.split_hover = None;
        self.column_hover = None;
        self.scroll_hover = None;
        if drag_moved {
            UiCommand::SaveState
        } else {
            UiCommand::None
        }
    }

    // 5-2-1. 화면 크기 반영 (그리기와 크기 변경 이벤트 공용)
    // 최소 크기 미달이 되면 열려 있던 종료 확인을 취소, 반환 = 확인을 취소했는지
    pub fn apply_screen_size(&mut self, width: u16, height: u16) -> bool {
        self.too_small = width < MIN_WIDTH || height < MIN_HEIGHT;
        if self.too_small && self.kill_confirm.take().is_some() {
            self.show_toast(i18n::text(self.lang, Msg::ToastKillCancelled), ToastLevel::Info);
            return true;
        }
        false
    }

    // 5-3. 키 입력 대상 카드 (클릭한 카드가 이 탭 것이면 그 카드, 아니면 탭의 주 카드)
    pub fn focused_card_id(&self) -> CardId {
        match self.focused_card {
            Some((tab, card)) if tab == self.tab => card,
            _ => self.tab.main_card(),
        }
    }

    // 5-4. 알림 띄우기·만료
    pub fn show_toast(&mut self, text: &str, level: ToastLevel) {
        self.toast = Some(Toast {
            text: text.to_string(),
            level,
            expires_at: self.now + TOAST_LIFETIME,
        });
    }

    // 만료 시각이 지났으면 지움, 반환 = 지웠는지
    pub fn expire_toast(&mut self, now: Instant) -> bool {
        let expired = self.toast.as_ref().is_some_and(|toast| now >= toast.expires_at);
        if expired {
            self.toast = None;
        }
        expired
    }

    // 5-5. 점멸 위상 (위험일 때만 화면에 보이므로 그때만 다시 그림), 반환 = 다시 그릴지
    pub fn tick_blink(&mut self, now: Instant) -> bool {
        if !self.blink_enabled || now.saturating_duration_since(self.last_blink) < BLINK_GAP {
            return false;
        }
        self.blink_on = !self.blink_on;
        self.last_blink = now;
        self.is_critical()
    }

    pub fn is_critical(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snap| snap.overall_risk >= RiskLevel::Critical)
    }

    // 갱신 지연 여부: 마지막 수신 뒤 폴링 주기의 3배 초과 (일시정지 중은 제외)
    pub fn stale_age_secs(&self) -> Option<u64> {
        let received = self.snapshot_at?;
        let age = self.now.saturating_duration_since(received);
        let limit = Duration::from_millis(self.poll_interval_ms * 3);
        (!self.paused && age > limit).then_some(age.as_secs())
    }

    // 5-6. 키 입력 상태 전이
    pub fn handle_key_event(&mut self, key: KeyEvent) -> UiCommand {
        // 1. Ctrl+C 종료
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return UiCommand::None;
        }

        // 2. 도움말은 아무 키로 닫기
        if self.show_help {
            self.show_help = false;
            return UiCommand::None;
        }

        // 3. 종료 확인은 Y 만 실행, 나머지는 취소
        if self.kill_confirm.is_some() {
            return match key.code {
                KeyCode::Char('y' | 'Y') => self.perform_action(UiAction::ConfirmKill),
                _ => self.perform_action(UiAction::CancelKill),
            };
        }

        // 4. 카드·탭 이동 중·길게 누름 대기 중에는 Esc 취소만 받음
        // 순서를 바꾸지 않은 탭 누름은 끝내고 키 처리, 단 Esc 는 끌기 취소로만 쓰여 종료로 넘어가지 않음
        if let Some(drag) = self.tab_drag
            && drag.baseline == self.tab_order
            && key.code != KeyCode::Esc
        {
            self.tab_drag = None;
        }
        if self.card_move.is_some() || self.card_press.is_some() || self.tab_drag.is_some() {
            if key.code == KeyCode::Esc {
                self.cancel_card_move();
                self.cancel_tab_drag();
            }
            return UiCommand::None;
        }

        // 5. 키별 전이 (번호 키·Tab 순환은 탭 바 표시 순서를 따름)
        let slot = self
            .tab_order
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or(0);
        let card = self.focused_card_id();
        match key.code {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => self.perform_action(UiAction::Quit),
            KeyCode::Char('?') => self.perform_action(UiAction::ShowHelp),
            KeyCode::Char('!') => self.perform_action(UiAction::ForceRestart),
            KeyCode::BackTab => self.switch_tab(self.tab_order[(slot + TAB_COUNT - 1) % TAB_COUNT]),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.switch_tab(self.tab_order[(slot + TAB_COUNT - 1) % TAB_COUNT])
            }
            KeyCode::Tab => self.switch_tab(self.tab_order[(slot + 1) % TAB_COUNT]),
            KeyCode::Char(digit @ '1'..='4') => {
                self.switch_tab(self.tab_order[digit as usize - '1' as usize])
            }
            KeyCode::Char('<') => self.move_current_tab(-1),
            KeyCode::Char('>') => self.move_current_tab(1),
            // Shift+방향키는 포커스 카드 위치 이동 (일반 방향키보다 먼저)
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
                if key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.shift_focused_card(card, key.code)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.step_focused_card(card, CursorMotion::Lines(-1))
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.step_focused_card(card, CursorMotion::Lines(1))
            }
            KeyCode::PageUp => self.step_focused_card(card, CursorMotion::Pages(-1)),
            KeyCode::PageDown => self.step_focused_card(card, CursorMotion::Pages(1)),
            KeyCode::Home => self.step_focused_card(card, CursorMotion::Home),
            KeyCode::End => self.step_focused_card(card, CursorMotion::End),
            KeyCode::Char(letter @ ('s' | 'S')) => {
                // 포커스 카드가 정렬 표가 아니면 탭의 주 카드
                let target = if card.sort_table().is_some() {
                    card
                } else {
                    self.tab.main_card()
                };
                let change = if letter == 's' {
                    SortChange::NextColumn
                } else {
                    SortChange::ReverseDirection
                };
                self.change_card_sort(target, change);
                UiCommand::None
            }
            KeyCode::Char('r' | 'R') => self.perform_action(UiAction::Refresh),
            KeyCode::Char(' ' | 'p' | 'P') => self.perform_action(UiAction::TogglePause),
            KeyCode::Char('x' | 'X') | KeyCode::F(9) | KeyCode::F(12) => {
                self.perform_action(UiAction::EmergencyStop)
            }
            KeyCode::Char('t' | 'T') => self.perform_action(UiAction::TogglePin),
            KeyCode::Char('b' | 'B') => self.perform_action(UiAction::ToggleBlink),
            KeyCode::Char('K') => self.perform_action(UiAction::KillSelected),
            KeyCode::Char('+' | '=') => self.change_interval(POLL_STEP_MS as i64),
            KeyCode::Char('-' | '_') => self.change_interval(-(POLL_STEP_MS as i64)),
            KeyCode::Char('l') => self.perform_action(UiAction::ResetLayout),
            KeyCode::Char('L') => self.perform_action(UiAction::ToggleLanguage),
            _ => UiCommand::None,
        }
    }

    // 폴링 주기 조절 (250ms 단위, 250~5000ms)
    fn change_interval(&mut self, delta: i64) -> UiCommand {
        let wanted = self.poll_interval_ms as i64 + delta;
        self.poll_interval_ms = wanted.clamp(POLL_MIN_MS as i64, POLL_MAX_MS as i64) as u64;
        self.show_toast(
            &i18n::interval_changed(self.lang, self.poll_interval_ms),
            ToastLevel::Info,
        );
        UiCommand::SetInterval(self.poll_interval_ms)
    }

    // 5-7. 키·버튼 공용 동작
    pub fn perform_action(&mut self, action: UiAction) -> UiCommand {
        match action {
            UiAction::Quit => {
                self.should_quit = true;
                UiCommand::None
            }
            UiAction::ShowHelp => {
                self.show_help = true;
                UiCommand::None
            }
            UiAction::Refresh => {
                self.show_toast(i18n::text(self.lang, Msg::ToastRefresh), ToastLevel::Info);
                UiCommand::RefreshNow
            }
            UiAction::TogglePause => {
                self.paused = !self.paused;
                let toast = if self.paused {
                    Msg::ToastPaused
                } else {
                    Msg::ToastResumed
                };
                self.show_toast(i18n::text(self.lang, toast), ToastLevel::Info);
                UiCommand::SetPaused(self.paused)
            }
            UiAction::TogglePin => {
                self.always_on_top = !self.always_on_top;
                let toast = if self.always_on_top {
                    Msg::ToastPinned
                } else {
                    Msg::ToastUnpinned
                };
                self.show_toast(i18n::text(self.lang, toast), ToastLevel::Info);
                UiCommand::SetTopmost(self.always_on_top)
            }
            UiAction::ToggleBlink => {
                self.blink_enabled = !self.blink_enabled;
                self.blink_on = false;
                let toast = if self.blink_enabled {
                    Msg::ToastBlinkOn
                } else {
                    Msg::ToastBlinkOff
                };
                self.show_toast(i18n::text(self.lang, toast), ToastLevel::Info);
                UiCommand::None
            }
            UiAction::ResetLayout => {
                self.card_layout.reset_groups(self.tab.split_groups());
                for card in self.tab.table_cards() {
                    self.column_widths[*card as usize] = [None; MAX_TABLE_COLUMNS];
                }
                self.split_drag = None;
                self.split_hover = None;
                self.column_drag = None;
                self.column_hover = None;
                self.show_toast(i18n::text(self.lang, Msg::ToastLayoutReset), ToastLevel::Info);
                UiCommand::SaveState
            }
            UiAction::ToggleLanguage => {
                self.lang = self.lang.toggled();
                self.show_toast(i18n::text(self.lang, Msg::ToastLanguage), ToastLevel::Info);
                UiCommand::SaveState
            }
            // 새 빌드가 없으면 칩이 없으므로 키도 무시
            UiAction::ForceRestart if self.exe_replaced => {
                self.restart_requested = true;
                self.should_quit = true;
                UiCommand::None
            }
            UiAction::ForceRestart => UiCommand::None,
            // 긴급 중단도 즉시 실행하지 않고 위험 등급 대상을 보여 주는 확인 창을 연다 (최소 크기 미달에서는 무시)
            UiAction::EmergencyStop => {
                if self.too_small {
                    return UiCommand::None;
                }
                let Some(snap) = self.snapshot.clone() else {
                    return UiCommand::None;
                };
                let targets: Vec<ProcessSummary> = snap
                    .processes
                    .iter()
                    .filter(|process| process.risk_level >= RiskLevel::Critical)
                    .cloned()
                    .collect();
                if targets.is_empty() {
                    self.show_toast(i18n::text(self.lang, Msg::ToastNoCritical), ToastLevel::Info);
                    return UiCommand::None;
                }
                self.kill_confirm = Some(KillConfirm::Emergency(targets));
                UiCommand::None
            }
            UiAction::KillSelected => {
                // 프로세스 표가 보이는 탭에서만, 화면 정렬 순서 기준 선택 행, 최소 크기 미달에서는 무시
                if !self.too_small && matches!(self.tab, ActiveTab::Overview | ActiveTab::Processes) {
                    self.kill_confirm = self.selected_process().cloned().map(|process| KillConfirm::Selected(Box::new(process)));
                }
                UiCommand::None
            }
            UiAction::ConfirmKill => {
                // 최소 크기 미달에서는 남아 있는 확인도 실행하지 않고 취소
                if self.too_small {
                    self.kill_confirm = None;
                    return UiCommand::None;
                }
                let Some(target) = self.kill_confirm.take() else {
                    return UiCommand::None;
                };
                match target {
                    KillConfirm::Selected(process) => {
                        self.show_toast(
                            i18n::text(self.lang, Msg::ToastKillRequested),
                            ToastLevel::Info,
                        );
                        UiCommand::KillProcess {
                            pid: process.pid,
                            name: process.process_name,
                        }
                    }
                    KillConfirm::Emergency(processes) => {
                        self.show_toast(
                            i18n::text(self.lang, Msg::ToastStopRequested),
                            ToastLevel::Info,
                        );
                        UiCommand::EmergencyStop(processes)
                    }
                }
            }
            UiAction::CancelKill => {
                self.kill_confirm = None;
                self.show_toast(i18n::text(self.lang, Msg::ToastKillCancelled), ToastLevel::Info);
                UiCommand::None
            }
        }
    }

    // 5-8. 선택 프로세스 (프로세스 표의 선택 행, 정렬 순서 기준)
    pub fn selected_process(&self) -> Option<&ProcessSummary> {
        let snap = self.snapshot.as_ref()?;
        let view = sort::processes(snap, &self.sorts);
        let selected = self.card_views[CardId::Processes as usize].selected;
        view.get(selected.min(view.len().saturating_sub(1))).copied()
    }

    // 5-9. 카드 정렬 변경 (머리글 클릭·s·S 공용, 선택 행은 같은 항목을 따라감)
    pub fn change_card_sort(&mut self, card: CardId, change: SortChange) {
        let Some(table) = card.sort_table() else {
            return;
        };
        let keys = self.capture_selection_keys();
        self.sorts.apply_change(table, change);
        self.restore_selection(keys);
        if card.kind() == CardKind::Scrollable {
            self.card_views[card as usize].scroll = 0;
        }
    }

    // 5-10. 키 이동 (선택 카드는 선택 행과 화면 위치, 그 외는 화면 위치만)
    pub fn step_focused_card(&mut self, card: CardId, motion: CursorMotion) -> UiCommand {
        let view = &mut self.card_views[card as usize];
        let page = view.visible.max(1) as isize;
        let last_row = view.total.saturating_sub(1);
        let max_scroll = view.total.saturating_sub(view.visible);
        let selectable = card.kind() == CardKind::Selectable;
        let current = if selectable { view.selected } else { view.scroll };
        let target = match motion {
            CursorMotion::Lines(delta) => current.saturating_add_signed(delta),
            CursorMotion::Pages(pages) => current.saturating_add_signed(pages * page),
            CursorMotion::Home => 0,
            CursorMotion::End => usize::MAX,
        };
        if selectable {
            view.selected = target.min(last_row);
            if view.selected < view.scroll {
                view.scroll = view.selected;
            } else if view.visible > 0 && view.selected >= view.scroll + view.visible {
                view.scroll = view.selected + 1 - view.visible;
            }
        } else {
            view.scroll = target.min(max_scroll);
        }
        UiCommand::None
    }

    // 5-11. 휠 스크롤 (선택 행은 그대로, 화면 위치만), 반환 = 위치가 바뀌었는지
    pub fn scroll_card_view(&mut self, card: CardId, delta: isize) -> bool {
        if card.kind() == CardKind::Static {
            return false;
        }
        let view = &mut self.card_views[card as usize];
        let max_scroll = view.total.saturating_sub(view.visible);
        let before = view.scroll;
        view.scroll = view.scroll.saturating_add_signed(delta).min(max_scroll);
        before != view.scroll
    }

    // 썸 드래그용 화면 위치 지정 (고정 카드 무시)
    pub fn set_card_scroll(&mut self, card: CardId, scroll: usize) {
        if card.kind() == CardKind::Static {
            return;
        }
        let view = &mut self.card_views[card as usize];
        view.scroll = scroll.min(view.total.saturating_sub(view.visible));
    }

    // 표 행 클릭 선택
    pub fn select_card_row(&mut self, card: CardId, index: usize) {
        let view = &mut self.card_views[card as usize];
        view.selected = index.min(view.total.saturating_sub(1));
    }

    // 5-12. 배경 스레드 메시지 반영, 반환 = 실행 요청 (위험 감지 시 창 올리기)
    pub fn apply_ui_message(&mut self, message: UiMessage, now: Instant) -> UiCommand {
        self.now = now;
        match message {
            UiMessage::Snapshot(snapshot) => {
                let keys = self.capture_selection_keys();
                let risk = snapshot.overall_risk;
                self.snapshot = Some(Arc::new(*snapshot));
                self.snapshot_at = Some(now);
                self.restore_selection(keys);

                // 위험 지속 시 6초마다 창을 올리고, 등급이 오르면 즉시
                let mut command = UiCommand::None;
                if risk >= RiskLevel::Critical {
                    let due = self
                        .last_elevate
                        .is_none_or(|at| now.saturating_duration_since(at) >= ELEVATE_REPEAT_GAP);
                    if risk > self.prev_risk || due {
                        self.always_on_top = true;
                        self.last_elevate = Some(now);
                        command = UiCommand::Elevate;
                    }
                }
                self.prev_risk = risk;
                command
            }
            UiMessage::StopDone(report) => {
                let level = if report.outcome.is_ok() {
                    ToastLevel::Success
                } else {
                    ToastLevel::Error
                };
                let detail = i18n::stop_report_text(self.lang, &report);
                self.show_toast(&i18n::stop_done(self.lang, &detail), level);
                UiCommand::None
            }
            UiMessage::KillDone { pid, name, result } => {
                match result {
                    Ok(()) => {
                        self.show_toast(&i18n::kill_done(self.lang, &name, pid), ToastLevel::Success);
                    }
                    Err(error) => {
                        let detail = i18n::kill_error_text(self.lang, &error);
                        self.show_toast(&i18n::kill_failed(self.lang, &detail), ToastLevel::Error);
                    }
                }
                UiCommand::None
            }
            UiMessage::StateSaveFailed { detail } => {
                self.show_toast(&i18n::state_save_failed(self.lang, &detail), ToastLevel::Error);
                UiCommand::None
            }
            UiMessage::ExeReplaced => {
                self.exe_replaced = true;
                self.show_toast(i18n::text(self.lang, Msg::ToastExeReplaced), ToastLevel::Info);
                UiCommand::None
            }
        }
    }

    // 5-13. 카드별 화면 순서 행 키 (현재 정렬 기준, 스냅샷 전은 None)
    fn card_row_keys(&self, card: CardId) -> Option<Vec<String>> {
        let snap = self.snapshot.as_ref()?;
        match card {
            CardId::Processes => Some(
                sort::processes(snap, &self.sorts)
                    .iter()
                    .map(|item| format!("{}\u{1}{}", item.pid, item.process_name))
                    .collect(),
            ),
            CardId::TopSqlRanking => Some(
                sort::top_sql(snap, &self.sorts)
                    .iter()
                    .map(|item| item.fingerprint.clone())
                    .collect(),
            ),
            CardId::AlertLog => Some(
                sort::alerts(snap, &self.sorts)
                    .iter()
                    .map(|item| item.id.to_string())
                    .collect(),
            ),
            _ => None,
        }
    }

    // 교체·정렬 전 선택 항목 키
    fn capture_selection_keys(&self) -> Vec<(CardId, Option<String>)> {
        SELECTABLE_CARDS
            .iter()
            .map(|card| {
                let selected = self.card_views[*card as usize].selected;
                let key = self
                    .card_row_keys(*card)
                    .and_then(|keys| keys.get(selected).cloned());
                (*card, key)
            })
            .collect()
    }

    // 교체·정렬 뒤 같은 항목으로 선택·화면 위치 이동 (못 찾으면 유지)
    fn restore_selection(&mut self, keys: Vec<(CardId, Option<String>)>) {
        for (card, key) in keys {
            let Some(key) = key else {
                continue;
            };
            let Some(row_keys) = self.card_row_keys(card) else {
                continue;
            };
            let Some(new_index) = row_keys.iter().position(|row_key| *row_key == key) else {
                continue;
            };
            let view = &mut self.card_views[card as usize];
            let shift = new_index as isize - view.selected as isize;
            view.scroll = view.scroll.saturating_add_signed(shift);
            view.selected = new_index;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    // 1. L 과 l 구분
    #[test]
    fn upper_l_switches_language_and_lower_l_resets_layout_without_aliasing() {
        let mut state = AppState::new(1000);
        assert_eq!(state.lang, Lang::En);
        let command = state.handle_key_event(key(KeyCode::Char('L')));
        assert!(matches!(command, UiCommand::SaveState));
        assert_eq!(state.lang, Lang::Ko);

        // 배치 초기화는 언어를 건드리지 않음
        state
            .card_layout
            .restore_ratios(SplitGroup::MetricRows, &[5_000]);
        let command = state.handle_key_event(key(KeyCode::Char('l')));
        assert!(matches!(command, UiCommand::SaveState));
        assert_eq!(state.lang, Lang::Ko);
        assert_eq!(state.card_layout, CardLayout::default());

        state.handle_key_event(key(KeyCode::Char('L')));
        assert_eq!(state.lang, Lang::En);
    }

    // 2. 키 표준 동작
    #[test]
    fn standard_keys_switch_tabs_toggle_help_and_quit() {
        let mut state = AppState::new(1000);
        state.handle_key_event(key(KeyCode::Tab));
        assert_eq!(state.tab, ActiveTab::Processes);
        state.handle_key_event(key(KeyCode::BackTab));
        assert_eq!(state.tab, ActiveTab::Overview);
        state.handle_key_event(key(KeyCode::Char('4')));
        assert_eq!(state.tab, ActiveTab::Alerts);
        state.handle_key_event(key(KeyCode::Char('?')));
        assert!(state.show_help);
        // 도움말은 아무 키로 닫고 그 키는 소비
        state.handle_key_event(key(KeyCode::Char('q')));
        assert!(!state.show_help && !state.should_quit);
        state.handle_key_event(key(KeyCode::Char('q')));
        assert!(state.should_quit);
    }

    // 3. 폴링 주기 범위
    #[test]
    fn interval_keys_step_by_250ms_within_range() {
        let mut state = AppState::new(1000);
        assert!(matches!(
            state.handle_key_event(key(KeyCode::Char('+'))),
            UiCommand::SetInterval(1250)
        ));
        state.poll_interval_ms = 5000;
        state.handle_key_event(key(KeyCode::Char('+')));
        assert_eq!(state.poll_interval_ms, 5000);
        state.poll_interval_ms = 250;
        state.handle_key_event(key(KeyCode::Char('-')));
        assert_eq!(state.poll_interval_ms, 250);
    }

    // 4. 알림 만료
    #[test]
    fn toast_expires_after_four_seconds_on_the_injected_clock() {
        let mut state = AppState::new(1000);
        let start = state.now;
        state.show_toast("x", ToastLevel::Info);
        assert!(!state.expire_toast(start + Duration::from_millis(3_999)));
        assert!(state.toast.is_some());
        assert!(state.expire_toast(start + Duration::from_secs(4)));
        assert!(state.toast.is_none());
    }

    // 5. 갱신 신선도
    #[test]
    fn staleness_starts_after_three_poll_intervals_and_never_applies_while_paused() {
        let mut state = AppState::new(1000);
        let start = state.now;
        assert_eq!(state.stale_age_secs(), None);
        state.snapshot_at = Some(start);
        state.now = start + Duration::from_millis(3_000);
        assert_eq!(state.stale_age_secs(), None);
        state.now = start + Duration::from_millis(3_001);
        assert_eq!(state.stale_age_secs(), Some(3));
        state.paused = true;
        assert_eq!(state.stale_age_secs(), None);
    }

    // 6. 강제 재시작
    #[test]
    fn bang_restarts_only_after_the_exe_was_replaced_and_plain_quit_never_requests_a_restart() {
        let mut state = AppState::new(1000);
        assert!(matches!(state.handle_key_event(key(KeyCode::Char('!'))), UiCommand::None));
        assert!(!state.should_quit && !state.restart_requested);

        // 교체 감지 뒤에는 재시작 표시와 함께 종료
        state.apply_ui_message(UiMessage::ExeReplaced, state.now);
        assert!(state.exe_replaced);
        assert_eq!(state.toast.as_ref().unwrap().level, ToastLevel::Info);
        state.handle_key_event(key(KeyCode::Char('!')));
        assert!(state.should_quit && state.restart_requested);

        // 일반 종료는 재시작 표시 없음
        let mut quitting = AppState::new(1000);
        quitting.apply_ui_message(UiMessage::ExeReplaced, quitting.now);
        quitting.handle_key_event(key(KeyCode::Char('q')));
        assert!(quitting.should_quit && !quitting.restart_requested);
    }
}
