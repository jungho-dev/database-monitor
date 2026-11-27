//! tui_charts.rs
//! 막대 차트 카드와 지표 카드 스파크라인 검증: 행 구성, 렌더 버퍼 셀 글자·색, 빈 상태.

#[allow(dead_code)]
mod common;

use std::sync::Arc;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::Span;
use database_monitor::analysis::risk::format_bytes;
use database_monitor::app::model::{MonitorSnapshot, RiskLevel, SqlFingerprintStats, SqlOpKind};
use database_monitor::tui::cards::CardId;
use database_monitor::tui::i18n::{self, Lang, Msg};
use database_monitor::tui::state::{ActiveTab, AppState};
use database_monitor::tui::theme;
use database_monitor::tui::views::charts::{
    BarRow, expand_trend, render_sql_mix, render_top_tables, sql_mix_rows, top_table_rows,
};
use database_monitor::tui::views::render_frame;
use common::build_fixture_snapshot;

const LANGS: [Lang; 2] = [Lang::En, Lang::Ko];

// 1. 공용 도구 ----------------------------------------------------------------
// 지문 통계를 (작업, 테이블, 실행 수, 수신 바이트, 위험도) 목록으로 바꾼 스냅샷
fn snapshot_with(entries: &[(SqlOpKind, &str, u64, u64, RiskLevel)]) -> MonitorSnapshot {
    let mut snap = build_fixture_snapshot(true);
    let base = snap.top_sql[0].clone();
    snap.top_sql = entries
        .iter()
        .map(
            |(op_kind, table, exec_count, rx_bytes, risk_level)| SqlFingerprintStats {
                fingerprint: format!("{table} {exec_count}"),
                op_kind: *op_kind,
                target_table: (*table).to_string(),
                exec_count: *exec_count,
                total_rx_bytes: *rx_bytes,
                risk_level: *risk_level,
                ..base.clone()
            },
        )
        .collect();
    snap
}

// 막대 카드 검증용: 9자 이름 작업 2개, 경고 테이블 1개, 이름 없는 테이블 1개
fn bar_snapshot() -> MonitorSnapshot {
    snapshot_with(&[
        (
            SqlOpKind::ShowMeta,
            "ORDERS",
            1200,
            3 * 1024 * 1024,
            RiskLevel::Warning,
        ),
        (
            SqlOpKind::PrepareExec,
            "USERS",
            800,
            1024 * 1024,
            RiskLevel::Safe,
        ),
        (SqlOpKind::Select, "", 300, 2048, RiskLevel::Safe),
    ])
}

fn draw_chart(snap: MonitorSnapshot, card: CardId, lang: Lang, width: u16, height: u16) -> Buffer {
    let snap = Arc::new(snap);
    let mut state = AppState::new(1000);
    state.lang = lang;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            if card == CardId::ChartSqlMix {
                render_sql_mix(frame, area, &mut state, &snap);
            } else {
                render_top_tables(frame, area, &mut state, &snap);
            }
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn draw_overview(snap: MonitorSnapshot, lang: Lang, width: u16, height: u16) -> (Buffer, AppState) {
    let mut state = AppState::new(1000);
    state.tab = ActiveTab::Overview;
    state.lang = lang;
    state.snapshot = Some(Arc::new(snap));
    state.snapshot_at = Some(state.now);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| render_frame(frame, &mut state))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (buffer, state)
}

// 넓은 글자의 이어지는 칸을 건너뛴 줄 목록
fn dump(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let mut row = String::new();
            let mut x = 0;
            while x < area.width {
                let symbol = buffer[(x, y)].symbol();
                row.push_str(symbol);
                x += Span::raw(symbol).width().max(1) as u16;
            }
            row
        })
        .collect()
}

fn has_text(buffer: &Buffer, needle: &str) -> bool {
    dump(buffer).iter().any(|row| row.contains(needle))
}

// 글자가 symbol 인 칸들의 글자색
fn symbol_colors(buffer: &Buffer, symbol: &str) -> Vec<Color> {
    buffer
        .content()
        .iter()
        .filter(|cell| cell.symbol() == symbol)
        .map(|cell| cell.fg)
        .collect()
}

fn card_inner(state: &AppState, card: CardId) -> Rect {
    let rect = state
        .hit_areas
        .card_rects
        .iter()
        .find(|(id, _)| *id == card)
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("{card:?} 카드 영역 없음"));
    Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2)
}

// 스파크라인 줄(안쪽 둘째 행)의 블록 글자 칸 (x, 글자색)
fn spark_blocks(buffer: &Buffer, inner: Rect) -> Vec<(u16, Color)> {
    let row_y = inner.y + 1;
    (inner.x..inner.right())
        .filter(|x| {
            let symbol = buffer[(*x, row_y)].symbol();
            symbol
                .chars()
                .all(|ch| ('\u{2581}'..='\u{2588}').contains(&ch))
                && !symbol.is_empty()
        })
        .map(|x| (x, buffer[(x, row_y)].fg))
        .collect()
}

// 2-1. 행 구성 ----------------------------------------------------------------
#[test]
fn sql_mix_rows_sum_per_kind_and_order_by_count_then_declaration() {
    let snap = snapshot_with(&[
        (SqlOpKind::Delete, "A", 10, 0, RiskLevel::Safe),
        (SqlOpKind::Select, "A", 10, 0, RiskLevel::Safe),
        (SqlOpKind::Insert, "B", 10, 0, RiskLevel::Safe),
        (SqlOpKind::Select, "B", 20, 0, RiskLevel::Safe),
        (SqlOpKind::Update, "C", 0, 0, RiskLevel::Safe),
    ]);
    let row = |label: &str, value: u64, value_text: &str| BarRow {
        label: label.to_string(),
        value,
        value_text: value_text.to_string(),
        flag: String::new(),
    };
    assert_eq!(
        sql_mix_rows(&snap),
        vec![
            row("SELECT", 30, "30  60%"),
            row("INSERT", 10, "10  20%"),
            row("DELETE", 10, "10  20%"),
        ]
    );
}

// 2-2. 행 구성 ----------------------------------------------------------------
#[test]
fn top_table_rows_group_by_table_with_dash_name_and_warning_flag() {
    let snap = snapshot_with(&[
        (SqlOpKind::Select, "USERS", 1, 4000, RiskLevel::Safe),
        (SqlOpKind::Select, "ORDERS", 1, 3000, RiskLevel::Warning),
        (SqlOpKind::Insert, "ORDERS", 1, 1000, RiskLevel::Critical),
        (SqlOpKind::Select, "", 1, 4000, RiskLevel::Critical),
        (SqlOpKind::Select, "ALPHA", 1, 100, RiskLevel::Caution),
    ]);
    let rows = top_table_rows(&snap);
    let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(labels, vec!["-", "ORDERS", "USERS", "ALPHA"]);
    let flags: Vec<&str> = rows.iter().map(|row| row.flag.as_str()).collect();
    assert_eq!(flags, vec!["!1", "!2", "", ""]);
    assert_eq!(rows[1].value, 4000);
    assert_eq!(rows[1].value_text, format_bytes(4000));
    assert_eq!(rows[3].value_text, format_bytes(100));
}

// 2-3. 행 구성 ----------------------------------------------------------------
#[test]
fn expand_trend_repeats_samples_to_fill_width_and_keeps_tail() {
    assert_eq!(
        expand_trend(&[1, 2, 3], 10),
        vec![1, 1, 1, 2, 2, 2, 3, 3, 3]
    );
    let history: Vec<u64> = (1..=60).collect();
    let expected: Vec<u64> = (41..=60).collect();
    assert_eq!(expand_trend(&history, 20), expected);
    assert!(expand_trend(&[], 20).is_empty());
    assert_eq!(expand_trend(&[5, 6], 1), vec![6]);
}

// 3-1. 막대 카드 --------------------------------------------------------------
#[test]
fn bar_cards_draw_names_values_fill_and_track_at_44_wide() {
    let cases = [
        (CardId::ChartSqlMix, Msg::CardSqlMix, theme::CHART_SQL_MIX),
        (
            CardId::ChartTopTables,
            Msg::CardTopTables,
            theme::CHART_TABLES,
        ),
    ];
    for lang in LANGS {
        for (card, title, color) in cases {
            let buffer = draw_chart(bar_snapshot(), card, lang, 44, 9);
            let snap = bar_snapshot();
            let rows = if card == CardId::ChartSqlMix {
                sql_mix_rows(&snap)
            } else {
                top_table_rows(&snap)
            };
            assert!(
                has_text(&buffer, i18n::text(lang, title)),
                "{card:?} {lang:?} 제목"
            );
            for row in &rows {
                assert!(
                    has_text(&buffer, &row.label),
                    "{card:?} {lang:?} {}",
                    row.label
                );
                assert!(
                    has_text(&buffer, &row.value_text),
                    "{card:?} {lang:?} {}",
                    row.value_text
                );
            }
            let fills = symbol_colors(&buffer, "█");
            assert!(
                !fills.is_empty() && fills.iter().all(|fg| *fg == color),
                "{card:?} 채움"
            );
            let tracks = symbol_colors(&buffer, "░");
            assert!(
                !tracks.is_empty() && tracks.iter().all(|fg| *fg == theme::FG_FAINT),
                "{card:?} 트랙"
            );
        }
    }
}

// 3-2. 막대 카드 --------------------------------------------------------------
#[test]
fn bar_cards_drop_the_bar_before_names_at_24_wide() {
    for lang in LANGS {
        for card in [CardId::ChartSqlMix, CardId::ChartTopTables] {
            let buffer = draw_chart(bar_snapshot(), card, lang, 24, 9);
            let labels: &[&str] = if card == CardId::ChartSqlMix {
                &["SHOW/META", "PREP/EXEC", "SELECT"]
            } else {
                &["ORDERS", "USERS", "-"]
            };
            for label in labels {
                assert!(
                    has_text(&buffer, &format!("{label} ")),
                    "{card:?} {lang:?} {label}"
                );
            }
            assert!(symbol_colors(&buffer, "█").is_empty(), "{card:?} 채움 없음");
            assert!(symbol_colors(&buffer, "░").is_empty(), "{card:?} 트랙 없음");
        }
    }
}

// 3-3. 막대 카드 --------------------------------------------------------------
#[test]
fn top_tables_warning_flag_uses_the_critical_color() {
    for lang in LANGS {
        for width in [44, 24] {
            let buffer = draw_chart(bar_snapshot(), CardId::ChartTopTables, lang, width, 9);
            assert!(has_text(&buffer, "!1"), "{lang:?} {width} 경고 꼬리");
            let flags = symbol_colors(&buffer, "!");
            assert!(
                !flags.is_empty() && flags.iter().all(|fg| *fg == theme::COLOR_CRIT),
                "{lang:?} {width} 경고 색"
            );
        }
    }
}

// 3-4. 막대 카드 --------------------------------------------------------------
#[test]
fn bar_cards_space_rows_when_room_and_skip_rows_beyond_height() {
    for lang in LANGS {
        let rows: Vec<usize> = dump(&draw_chart(
            bar_snapshot(),
            CardId::ChartSqlMix,
            lang,
            44,
            9,
        ))
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            ["SHOW/META ", "PREP/EXEC ", "SELECT "]
                .iter()
                .any(|label| row.contains(label))
        })
        .map(|(index, _)| index)
        .collect();
        assert_eq!(rows, vec![1, 3, 5], "{lang:?} 행 사이 빈 줄");
        let crowded = snapshot_with(&[
            (SqlOpKind::Select, "A", 50, 0, RiskLevel::Safe),
            (SqlOpKind::Insert, "A", 40, 0, RiskLevel::Safe),
            (SqlOpKind::Update, "A", 30, 0, RiskLevel::Safe),
            (SqlOpKind::Delete, "A", 20, 0, RiskLevel::Safe),
            (SqlOpKind::Ddl, "A", 10, 0, RiskLevel::Safe),
        ]);
        let buffer = draw_chart(crowded, CardId::ChartSqlMix, lang, 44, 5);
        for label in ["SELECT", "INSERT", "UPDATE"] {
            assert!(has_text(&buffer, label), "{lang:?} {label}");
        }
        for label in ["DELETE", "DDL"] {
            assert!(!has_text(&buffer, label), "{lang:?} {label} 생략");
        }
    }
}

// 4. 빈 상태 ------------------------------------------------------------------
#[test]
fn empty_top_sql_shows_the_notice_without_bar_glyphs() {
    for lang in LANGS {
        for card in [CardId::ChartSqlMix, CardId::ChartTopTables] {
            let buffer = draw_chart(snapshot_with(&[]), card, lang, 44, 9);
            assert!(
                has_text(&buffer, i18n::text(lang, Msg::EmptyTopSql)),
                "{card:?} {lang:?}"
            );
            assert!(
                symbol_colors(&buffer, "█").is_empty(),
                "{card:?} {lang:?} 채움"
            );
            assert!(
                symbol_colors(&buffer, "░").is_empty(),
                "{card:?} {lang:?} 트랙"
            );
        }
    }
}

// 5-1. 스파크라인 -------------------------------------------------------------
#[test]
fn metric_sparklines_use_series_colors_with_legend_marks_right_aligned() {
    let reserved = [theme::BORDER_FOCUS, theme::FG_ACCENT];
    for lang in LANGS {
        let mut snap = build_fixture_snapshot(true);
        snap.rx_history_kb = (1..=60).collect();
        snap.qps_history = (1..=60).map(|sample| sample * 2).collect();
        let (buffer, state) = draw_overview(snap, lang, 130, 40);
        for (card, color) in [
            (CardId::Traffic, theme::CHART_RX),
            (CardId::QueryRate, theme::CHART_QPS),
        ] {
            let inner = card_inner(&state, card);
            let mark = &buffer[(inner.x, inner.y)];
            assert_eq!(mark.symbol(), "━", "{card:?} {lang:?} 범례 표식");
            assert_eq!(mark.fg, color, "{card:?} {lang:?} 범례 색");
            let blocks = spark_blocks(&buffer, inner);
            assert!(!blocks.is_empty(), "{card:?} {lang:?} 스파크라인");
            assert!(
                blocks.iter().all(|(_, fg)| *fg == color),
                "{card:?} {lang:?} 계열 색"
            );
            assert_eq!(
                blocks.last().map(|(x, _)| *x),
                Some(inner.right() - 1),
                "{card:?} 오른쪽 정렬"
            );
            for x in inner.x..inner.right() {
                let fg = buffer[(x, inner.y + 1)].fg;
                assert!(!reserved.contains(&fg), "{card:?} {lang:?} x={x} 예약 색");
            }
        }
    }
}

// 5-2. 스파크라인 -------------------------------------------------------------
#[test]
fn all_zero_history_keeps_the_legend_mark_without_spark_blocks() {
    for lang in LANGS {
        let mut snap = build_fixture_snapshot(true);
        snap.rx_history_kb = vec![0; 60];
        snap.qps_history = vec![0; 60];
        let (buffer, state) = draw_overview(snap, lang, 130, 40);
        for (card, color) in [
            (CardId::Traffic, theme::CHART_RX),
            (CardId::QueryRate, theme::CHART_QPS),
        ] {
            let inner = card_inner(&state, card);
            let mark = &buffer[(inner.x, inner.y)];
            assert_eq!(
                (mark.symbol(), mark.fg),
                ("━", color),
                "{card:?} {lang:?} 범례"
            );
            assert!(
                spark_blocks(&buffer, inner).is_empty(),
                "{card:?} {lang:?} 블록 없음"
            );
        }
    }
}

// 5-3. 최소 크기 값 줄 --------------------------------------------------------
#[test]
fn metric_value_lines_keep_full_values_and_plot_only_with_a_legend_mark_at_60x16() {
    for lang in LANGS {
        let mut snap = build_fixture_snapshot(true);
        snap.rx_history_kb = (1..=60).collect();
        snap.qps_history = (1..=60).collect();
        snap.total_rx_bps = 12_345_678;
        snap.total_qps = 1_234;
        let rx_text = format!("RX {}/s", format_bytes(snap.total_rx_bps));
        let qps_text = format!("{} QPS", snap.total_qps);
        let (buffer, state) = draw_overview(snap, lang, 60, 16);
        for (card, needle) in [(CardId::Traffic, &rx_text), (CardId::QueryRate, &qps_text)] {
            let inner = card_inner(&state, card);
            let row: String = (inner.x..inner.right())
                .map(|x| buffer[(x, inner.y)].symbol().to_string())
                .collect();
            // 주 값은 항상 온전히, 범례 표식이 있을 때만 스파크라인 (색만으로 구분되는 계열 없음)
            assert!(row.contains(needle.as_str()), "{card:?} {lang:?} 값 잘림: {row}");
            let has_mark = row.starts_with("━ ");
            let has_plot = !spark_blocks(&buffer, inner).is_empty();
            assert_eq!(has_mark, has_plot, "{card:?} {lang:?} 범례·계열: {row}");
        }
        // 좌은 트래픽 카드는 표식과 계열을 빼고, 넓은 쿼리 카드는 둘 다 유지
        let rx_inner = card_inner(&state, CardId::Traffic);
        assert_ne!(buffer[(rx_inner.x, rx_inner.y)].symbol(), "━", "{lang:?} 트래픽 표식 생략");
        let qps_inner = card_inner(&state, CardId::QueryRate);
        assert_eq!(buffer[(qps_inner.x, qps_inner.y)].symbol(), "━", "{lang:?} 쿼리 표식");
    }
}
