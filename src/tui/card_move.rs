//! card_move.rs
//! 카드 위치 이동: 제목 길게 누름 시계, 이동 중 미리보기, 확정·취소, Shift+방향키 한 칸 이동.

use std::time::{Duration, Instant};
use crossterm::event::KeyCode;
use ratatui::layout::Position;
use crate::tui::cards::{CARD_HOLD_TIME, CardId, CardMove};
use crate::tui::i18n::{self, Msg};
use crate::tui::layout::SplitAxis;
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
        let Some(slot) = self.card_layout.slot_of(press.group, press.index) else {
            return false;
        };
        self.card_move = Some(CardMove {
            card: press.card,
            group: press.group,
            index: press.index,
            baseline: self.card_layout.order_of(press.group),
            target: slot,
        });
        true
    }

    // 이동 모드 진입까지 남은 시간 (이벤트 대기 시간 상한 계산용), 대기 중이 아니면 None
    pub fn press_remaining(&self, now: Instant) -> Option<Duration> {
        self.card_press.map(|press| {
            CARD_HOLD_TIME.saturating_sub(now.saturating_duration_since(press.started))
        })
    }

    // 2. 카드 이동 취소·미리보기·확정 -----------------------------------------
    // 대기 중 누름은 버리고, 이동 중이면 누르기 전 순서로 복원, 반환 = 상태가 바뀌었는지
    pub fn cancel_card_move(&mut self) -> bool {
        let pressed = self.card_press.take().is_some();
        match self.card_move.take() {
            Some(moving) => {
                self.card_layout.set_order(moving.group, moving.baseline);
                true
            }
            None => pressed,
        }
    }

    // 포인터가 놓인 칸을 목표로 삼아 누르기 전 순서에서 삽입 이동을 다시 계산, 칸 밖이면 마지막 목표 유지
    pub fn preview_card_move(&mut self, position: Position) {
        let Some(moving) = self.card_move else {
            return;
        };
        let target = self
            .hit_areas
            .slot_groups
            .iter()
            .find(|slots| slots.group == moving.group)
            .and_then(|slots| slots.slots.iter().position(|slot| slot.contains(position)));
        let Some(target) = target else {
            return;
        };
        if target == moving.target {
            return;
        }
        self.card_layout
            .move_card_to_slot(moving.group, &moving.baseline, moving.index, target);
        self.card_move = Some(CardMove { target, ..moving });
    }

    // 순서가 바뀌었을 때만 알림과 저장 요청 (제자리에 놓으면 아무 일 없음)
    pub fn commit_card_move(&mut self) -> UiCommand {
        let Some(moving) = self.card_move.take() else {
            return UiCommand::None;
        };
        if self.card_layout.order_of(moving.group) == moving.baseline {
            return UiCommand::None;
        }
        self.show_toast(i18n::text(self.lang, Msg::ToastCardMoved), ToastLevel::Info);
        UiCommand::SaveState
    }

    // 3. Shift+방향키 한 칸 이동 ----------------------------------------------
    // 세로 나열 그룹은 ↑↓, 가로 나열 그룹은 ←→ 만 받고 나머지는 무시, 바로 확정·저장
    // 이번 프레임에 그려진 그룹만 대상 (안내 카드만 보이는 탭은 순서를 바꾸지 않음)
    pub fn shift_focused_card(&mut self, card: CardId, code: KeyCode) -> UiCommand {
        let Some((group, index)) = card.reorder_slot(self.tab) else {
            return UiCommand::None;
        };
        if !self
            .hit_areas
            .slot_groups
            .iter()
            .any(|slots| slots.group == group)
        {
            return UiCommand::None;
        }
        let delta: isize = match (group.axis(), code) {
            (SplitAxis::Rows, KeyCode::Up) | (SplitAxis::Columns, KeyCode::Left) => -1,
            (SplitAxis::Rows, KeyCode::Down) | (SplitAxis::Columns, KeyCode::Right) => 1,
            _ => return UiCommand::None,
        };
        let Some(slot) = self.card_layout.slot_of(group, index) else {
            return UiCommand::None;
        };
        let Some(target) = slot
            .checked_add_signed(delta)
            .filter(|target| *target < group.card_count())
        else {
            return UiCommand::None;
        };
        let baseline = self.card_layout.order_of(group);
        self.card_layout
            .move_card_to_slot(group, &baseline, index, target);
        self.show_toast(i18n::text(self.lang, Msg::ToastCardMoved), ToastLevel::Info);
        UiCommand::SaveState
    }
}
