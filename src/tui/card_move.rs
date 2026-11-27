//! card_move.rs
//! 카드 위치 이동: 제목 길게 누름 시계, 이동 중 미리보기, 확정·취소, Shift+방향키 한 칸 이동.

use std::time::{Duration, Instant};
use crossterm::event::KeyCode;
use ratatui::layout::{Position, Rect};
use crate::tui::cards::{CARD_HOLD_TIME, CardId, CardMove, GridLineHit};
use crate::tui::i18n::{self, Msg};
use crate::tui::layout::grid::{GridLine, GridMove, GridPlan};
use crate::tui::layout::grid::{apply_drop, resolve_drop, shift_card};
use crate::tui::state::{AppState, ToastLevel, UiCommand};

impl AppState {
    // 1. 길게 누름 시계 -------------------------------------------------------
    // 제목을 누른 지 CARD_HOLD_TIME 이 지나면 이동 모드 진입 (마우스 움직임과 무관), 반환 = 상태가 바뀌었는지
    pub fn tick(&mut self, now: Instant) -> bool {
        let Some(press) = self.card_press else {
            return false;
        };
        if now.saturating_duration_since(press.started) < CARD_HOLD_TIME {
            return false;
        }
        self.card_press = None;

        // 격자 카드: 누른 순간 격자와 줄 보관
        let grid_tab = self
            .hit_areas
            .grid_lines
            .iter()
            .find(|line| line.cards.iter().any(|(placed, _)| *placed == press.card))
            .map(|line| line.tab);
        let Some(tab) = grid_tab else {
            return false;
        };
        let lines: Vec<GridLineHit> = self
            .hit_areas
            .grid_lines
            .iter()
            .filter(|line| line.tab == tab)
            .cloned()
            .collect();
        let baseline = self.card_layout.begin_grid_move(tab);
        self.grid_move = Some(GridMove {
            tab,
            card: press.card,
            baseline,
            lines,
        });
        self.card_move = Some(CardMove { card: press.card });
        true
    }

    // 이동 모드 진입까지 남은 시간 (이벤트 대기 시간 상한 계산용), 대기 중이 아니면 None
    pub fn press_remaining(&self, now: Instant) -> Option<Duration> {
        self.card_press.map(|press| {
            CARD_HOLD_TIME.saturating_sub(now.saturating_duration_since(press.started))
        })
    }

    // 2. 취소 -----------------------------------------------------------------
    // 대기 중 누름은 버리고, 이동 중이면 누르기 전 격자로 복원, 반환 = 상태가 바뀌었는지
    pub fn cancel_card_move(&mut self) -> bool {
        let pressed = self.card_press.take().is_some();
        let lifted = self.card_move.take().is_some();
        let Some(moving) = self.grid_move.take() else {
            return pressed || lifted;
        };
        self.card_layout
            .end_grid_move(moving.tab, Some(moving.baseline));
        true
    }

    // 3. 미리보기 -------------------------------------------------------------
    // 격자 카드 미리보기: 누른 순간 줄 기준 놓을 자리, 거부되면 미리보기 유지
    pub fn preview_grid_move(&mut self, position: Position) {
        let Some(moving) = self.grid_move.as_ref() else {
            return;
        };

        // 1. 누른 순간 줄의 놓을 자리
        let plan = GridPlan {
            lines: moving
                .lines
                .iter()
                .map(|line| GridLine {
                    row: line.row,
                    cards: line.cards.iter().map(|(card, _)| *card).collect(),
                    height: line.rect.height,
                })
                .collect(),
        };
        let line_rects: Vec<Rect> = moving.lines.iter().map(|line| line.rect).collect();
        let card_rects: Vec<(CardId, Rect)> = moving
            .lines
            .iter()
            .flat_map(|line| line.cards.iter().copied())
            .collect();
        let target = resolve_drop(&plan, &line_rects, &card_rects, moving.card, position);

        // 2. 누르기 전 격자에서 다시 계산
        let (tab, baseline) = (moving.tab, moving.baseline);
        let current = self.card_layout.grid_of(tab);
        let Some(moved) =
            apply_drop(&baseline, moving.card, target).filter(|moved| *moved != current)
        else {
            return;
        };

        // 3. 제자리 복원 (비율 포함)
        if moved == baseline {
            self.card_layout.end_grid_move(tab, Some(baseline));
            self.card_layout.begin_grid_move(tab);
            return;
        }
        self.card_layout.preview_grid(tab, moved);
    }

    // 4. 확정 -----------------------------------------------------------------
    // 격자가 바뀌었을 때만 알림과 저장 요청 (제자리에 놓으면 아무 일 없음)
    pub fn commit_card_move(&mut self) -> UiCommand {
        // 1. 미리보기 격자, 누르기 전 격자로 복원
        self.card_move = None;
        let Some(moving) = self.grid_move.take() else {
            return UiCommand::None;
        };
        let committed = self.card_layout.grid_of(moving.tab);
        self.card_layout
            .end_grid_move(moving.tab, Some(moving.baseline));

        // 2. 바뀐 격자만 확정·알림·저장 (비율 초기화 한 번)
        if committed == moving.baseline {
            return UiCommand::None;
        }
        self.card_layout.set_grid(moving.tab, committed);
        self.show_toast(i18n::text(self.lang, Msg::ToastCardMoved), ToastLevel::Info);
        UiCommand::SaveState
    }

    // 5. Shift+방향키 한 칸 이동 ----------------------------------------------
    // 이번 프레임에 그려진 격자 카드만 대상 (안내 카드만 보이는 탭은 배치를 바꾸지 않음), 격자가 바뀔 때만 확정·저장
    pub fn shift_focused_card(&mut self, card: CardId, code: KeyCode) -> UiCommand {
        // 1. 이번 프레임 격자 카드
        let drawn_tab = self
            .hit_areas
            .grid_lines
            .iter()
            .find(|line| line.cards.iter().any(|(placed, _)| *placed == card))
            .map(|line| line.tab);
        let Some(tab) = drawn_tab else {
            return UiCommand::None;
        };
        let drawn: Vec<CardId> = self
            .hit_areas
            .grid_lines
            .iter()
            .filter(|line| line.tab == tab)
            .flat_map(|line| line.cards.iter().map(|(placed, _)| *placed))
            .collect();

        // 2. 격자 이동
        let grid = self.card_layout.grid_of(tab);
        let Some(moved) = shift_card(&grid, card, code, &drawn).filter(|moved| *moved != grid)
        else {
            return UiCommand::None;
        };
        self.card_layout.set_grid(tab, moved);
        self.show_toast(i18n::text(self.lang, Msg::ToastCardMoved), ToastLevel::Info);
        UiCommand::SaveState
    }
}
