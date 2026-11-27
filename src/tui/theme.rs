//! theme.rs
//! 256-컬러 다크 테마 팔레트: 서피스 단계, 텍스트, 상태, 스크롤바·분할선, 위험도·행위자·SQL 작업 스타일.
//! 외곽선 단계: 평소 240 둥근선, 호버 67 둥근선, 포커스 39 굵은선·반전 제목, 위험 203 최우선.

use ratatui::style::{Color, Modifier, Style};
use crate::app::model::{ActorKind, RiskLevel, SqlOpKind};

// 1. 서피스·외곽선 ------------------------------------------------------------
pub const BG_BASE: Color = Color::Indexed(233);
pub const BG_PANEL: Color = Color::Indexed(235);
pub const BG_BAR: Color = Color::Indexed(236);
pub const BG_HEADER: Color = Color::Indexed(237);
pub const BG_TONAL: Color = Color::Indexed(238);
pub const BG_SELECTED: Color = Color::Indexed(24);
pub const BG_SELECTED_IDLE: Color = Color::Indexed(237);
// 위험 점멸 시 바탕 색조
pub const BG_DANGER_TINT: Color = Color::Indexed(52);
pub const BORDER_DIM: Color = Color::Indexed(240);
pub const BORDER_FOCUS: Color = Color::Indexed(39);
// 마우스 아래 카드: 기본 외곽선과 선택 강조 사이 밝기
pub const BORDER_HOVER: Color = Color::Indexed(67);

// 2. 텍스트 -------------------------------------------------------------------
pub const FG_TEXT: Color = Color::Indexed(252);
pub const FG_STRONG: Color = Color::Indexed(231);
pub const FG_DIM: Color = Color::Indexed(247);
pub const FG_FAINT: Color = Color::Indexed(243);
pub const FG_ACCENT: Color = Color::Indexed(81);
pub const FG_ON_ACCENT: Color = Color::Indexed(16);
// 포커스·호버가 아닌 카드 제목, 호버 카드 제목, 단축키 글자, 비활성 탭 이름
pub const TITLE_IDLE: Color = Color::Indexed(250);
pub const TITLE_HOVER: Color = FG_TEXT;
pub const HINT_KEY: Color = Color::Indexed(250);
pub const TAB_IDLE_NAME: Color = Color::Indexed(248);

// 3. 상태 색 ------------------------------------------------------------------
pub const COLOR_OK: Color = Color::Indexed(78);
pub const COLOR_WARN: Color = Color::Indexed(220);
pub const COLOR_CRIT: Color = Color::Indexed(203);
// 관찰 등급은 위험이 아니라 눈에 덜 띄는 색
pub const COLOR_OBSERVE: Color = Color::Indexed(144);
// 오류 알림 채움 바탕 (흰 글자와 함께)
pub const COLOR_CRIT_BG: Color = Color::Indexed(160);
// 위험 경보 카드 점멸 두 위상 (바탕 적색 채움, 바탕 황색 채움)
pub const ALARM_FILL: Color = Color::Indexed(196);
pub const ALARM_FLASH: Color = Color::Indexed(226);
// 경보 카드 안 행동 안내 칸 바탕
pub const ALARM_LABEL_BG: Color = Color::Indexed(16);

// 4. 행위자·SQL 작업 색 -------------------------------------------------------
pub const COLOR_ACTOR_AGENT: Color = Color::Indexed(213);
pub const COLOR_ACTOR_SPRING: Color = Color::Indexed(117);
pub const COLOR_ACTOR_CLIENT: Color = Color::Indexed(150);
pub const COLOR_ACTOR_SCRIPT: Color = Color::Indexed(222);
pub const COLOR_SQL_SELECT: Color = Color::Indexed(81);
pub const COLOR_SQL_META: Color = Color::Indexed(147);

// 5. 스크롤바·분할선 색 -------------------------------------------------------
// 트랙: 평소·포커스, 호버·드래그 중
pub const SCROLL_TRACK: Color = Color::Indexed(238);
pub const SCROLL_TRACK_HOT: Color = Color::Indexed(241);
// 썸: 평소, 포커스 카드, 트랙 호버, 드래그 중 (외곽선 포커스색과 겹치지 않는 회색 계열)
pub const SCROLL_THUMB: Color = Color::Indexed(245);
pub const SCROLL_THUMB_FOCUS: Color = Color::Indexed(250);
pub const SCROLL_THUMB_HOVER: Color = Color::Indexed(254);
pub const SCROLL_THUMB_DRAG: Color = Color::Indexed(231);
// 분할선: 호버, 드래그 중 (활성일 때만 그림)
pub const SPLIT_GRIP_HOT: Color = Color::Indexed(250);
pub const SPLIT_GRIP_DRAG: Color = Color::Indexed(231);

// 6. 차트 계열 색 (계열마다 한 의미, 포커스·강조·상태 색과 겹치지 않음) -------
pub const CHART_RX: Color = Color::Indexed(173);
pub const CHART_QPS: Color = Color::Indexed(139);
// SQL 작업 색 중 SELECT(81)는 강조색과 같아 계열 색으로 못 쓰므로 구성 막대는 한 색
pub const CHART_SQL_MIX: Color = Color::Indexed(108);
pub const CHART_TABLES: Color = Color::Indexed(180);

// 7. 공용 스타일 --------------------------------------------------------------
pub const STYLE_TEXT: Style = Style::new().fg(FG_TEXT);
pub const STYLE_DIM: Style = Style::new().fg(FG_DIM);
pub const STYLE_FAINT: Style = Style::new().fg(FG_FAINT);
pub const STYLE_STRONG: Style = Style::new().fg(FG_STRONG).add_modifier(Modifier::BOLD);
pub const STYLE_ACCENT: Style = Style::new().fg(FG_ACCENT).add_modifier(Modifier::BOLD);
pub const STYLE_OK: Style = Style::new().fg(COLOR_OK);
pub const STYLE_WARN: Style = Style::new().fg(COLOR_WARN);
pub const STYLE_CRIT: Style = Style::new().fg(COLOR_CRIT);
pub const STYLE_TABLE_HEADER: Style = Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD);
pub const STYLE_SELECTED_ROW: Style = Style::new().bg(BG_SELECTED).fg(FG_STRONG);
// 포커스가 아닌 카드의 선택 행 (바탕만 칠해 위험도별 글자색 유지)
pub const STYLE_SELECTED_ROW_IDLE: Style = Style::new().bg(BG_SELECTED_IDLE);
pub const STYLE_BADGE: Style = Style::new()
    .fg(FG_ON_ACCENT)
    .bg(FG_ACCENT)
    .add_modifier(Modifier::BOLD);
pub const STYLE_BADGE_WARN: Style = Style::new()
    .fg(FG_ON_ACCENT)
    .bg(COLOR_WARN)
    .add_modifier(Modifier::BOLD);
// 오류 알림 칩 (유일한 적색 채움 알림)
pub const STYLE_TOAST_ERROR: Style = Style::new()
    .fg(FG_STRONG)
    .bg(COLOR_CRIT_BG)
    .add_modifier(Modifier::BOLD);
pub const STYLE_CHIP_KEY: Style = Style::new()
    .fg(FG_ACCENT)
    .bg(BG_TONAL)
    .add_modifier(Modifier::BOLD);
pub const STYLE_CHIP_NAME: Style = Style::new().fg(FG_TEXT).bg(BG_TONAL);
pub const STYLE_CHIP_QUIT_KEY: Style = Style::new()
    .fg(COLOR_CRIT)
    .bg(BG_TONAL)
    .add_modifier(Modifier::BOLD);
pub const STYLE_CHIP_ON: Style = Style::new()
    .fg(FG_ON_ACCENT)
    .bg(COLOR_WARN)
    .add_modifier(Modifier::BOLD);
// 파괴 확인: 채움 Y, 톤 N
pub const STYLE_CONFIRM_YES: Style = Style::new()
    .fg(FG_ON_ACCENT)
    .bg(COLOR_CRIT)
    .add_modifier(Modifier::BOLD);
pub const STYLE_CONFIRM_NO: Style = Style::new().fg(FG_TEXT).bg(BG_TONAL);

// 8. 위험도·행위자·SQL 작업 스타일 --------------------------------------------
pub fn risk_color(level: RiskLevel) -> Color {
    match level {
        RiskLevel::Safe => COLOR_OK,
        RiskLevel::Caution => COLOR_OBSERVE,
        RiskLevel::Warning => COLOR_WARN,
        RiskLevel::Critical => COLOR_CRIT,
    }
}

pub fn risk_badge_style(level: RiskLevel) -> Style {
    Style::new()
        .fg(FG_ON_ACCENT)
        .bg(risk_color(level))
        .add_modifier(Modifier::BOLD)
}

pub fn risk_text_style(level: RiskLevel) -> Style {
    match level {
        RiskLevel::Critical => Style::new().fg(COLOR_CRIT).add_modifier(Modifier::BOLD),
        other => Style::new().fg(risk_color(other)),
    }
}

pub fn actor_style(kind: ActorKind) -> Style {
    match kind {
        ActorKind::AiAgentMcp => Style::new()
            .fg(COLOR_ACTOR_AGENT)
            .add_modifier(Modifier::BOLD),
        ActorKind::JvmSpringApp => Style::new()
            .fg(COLOR_ACTOR_SPRING)
            .add_modifier(Modifier::BOLD),
        ActorKind::DbClientCli => Style::new().fg(COLOR_ACTOR_CLIENT),
        ActorKind::ScriptWorker => Style::new().fg(COLOR_ACTOR_SCRIPT),
        ActorKind::SystemOther => STYLE_DIM,
    }
}

pub fn sql_op_style(op: SqlOpKind) -> Style {
    match op {
        SqlOpKind::Select => Style::new().fg(COLOR_SQL_SELECT),
        SqlOpKind::Insert | SqlOpKind::Update => {
            Style::new().fg(COLOR_WARN).add_modifier(Modifier::BOLD)
        }
        SqlOpKind::Delete | SqlOpKind::Ddl => {
            Style::new().fg(COLOR_CRIT).add_modifier(Modifier::BOLD)
        }
        SqlOpKind::ShowMeta | SqlOpKind::PrepareExec => Style::new().fg(COLOR_SQL_META),
        SqlOpKind::Transaction => Style::new().fg(COLOR_WARN),
        SqlOpKind::PingQuit | SqlOpKind::EncryptedFlow | SqlOpKind::SocketConnect => STYLE_DIM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. 스크롤바 색 분리
    #[test]
    fn scroll_and_grip_colors_never_reuse_the_focus_or_accent_color() {
        for color in [
            SCROLL_THUMB,
            SCROLL_THUMB_FOCUS,
            SCROLL_THUMB_HOVER,
            SCROLL_THUMB_DRAG,
            SCROLL_TRACK,
            SCROLL_TRACK_HOT,
            SPLIT_GRIP_HOT,
            SPLIT_GRIP_DRAG,
        ] {
            assert_ne!(color, BORDER_FOCUS);
            assert_ne!(color, FG_ACCENT);
        }
    }

    // 2. 위험도 색 구분
    #[test]
    fn risk_levels_use_distinct_colors_and_critical_is_bold() {
        let levels = [
            RiskLevel::Safe,
            RiskLevel::Caution,
            RiskLevel::Warning,
            RiskLevel::Critical,
        ];
        for (index, left) in levels.iter().enumerate() {
            for right in &levels[index + 1..] {
                assert_ne!(risk_color(*left), risk_color(*right));
            }
        }
        assert_eq!(risk_color(RiskLevel::Critical), COLOR_CRIT);
        assert!(
            risk_text_style(RiskLevel::Critical)
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    // 3. 차트 계열 색 분리
    #[test]
    fn chart_series_colors_are_distinct_and_never_reuse_reserved_colors() {
        let series = [CHART_RX, CHART_QPS, CHART_SQL_MIX, CHART_TABLES];
        let reserved = [
            BORDER_FOCUS,
            BORDER_HOVER,
            FG_ACCENT,
            COLOR_OK,
            COLOR_WARN,
            COLOR_CRIT,
            COLOR_OBSERVE,
            COLOR_CRIT_BG,
            COLOR_ACTOR_AGENT,
            COLOR_ACTOR_SPRING,
            COLOR_ACTOR_CLIENT,
            COLOR_ACTOR_SCRIPT,
            COLOR_SQL_SELECT,
            COLOR_SQL_META,
        ];
        for (index, left) in series.iter().enumerate() {
            for right in &series[index + 1..] {
                assert_ne!(left, right);
            }
            assert!(!reserved.contains(left));
        }
    }
}
