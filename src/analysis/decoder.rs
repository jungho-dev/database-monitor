//! decoder.rs
//! 패시브 TCP 페이로드 기반 MySQL·PostgreSQL·Redis 프로토콜 디코더 및 SQL 정규화.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Instant;
use crate::app::config::MonitorConfig;
use crate::app::model::{DbEngine, RiskLevel, RiskReason, SqlOpKind};

// 1. 디코딩된 와이어 프레임 ---------------------------------------------------
#[derive(Debug, Clone)]
pub struct WireFrame {
    pub src_ip: IpAddr,
    pub src_port: u16,
    pub dst_ip: IpAddr,
    pub dst_port: u16,
    pub payload_len: usize,
    pub decoded_sql: Option<DecodedSql>,
    pub captured_at: Instant,
}

#[derive(Debug, Clone)]
pub struct DecodedSql {
    pub engine: DbEngine,
    pub op_kind: SqlOpKind,
    pub target_table: String,
    pub fingerprint: String,
    pub preview: String,
    pub full_sql: String,
    pub pattern_risk: RiskLevel,
    pub pattern_note: Option<RiskReason>,
}

// 2. L2/L3/L4 패킷 헤더 파싱 --------------------------------------------------
pub fn decode_packet(raw: &[u8], datalink: i32, watch: &MonitorConfig) -> Option<WireFrame> {
    // 1. 데이터링크 오프셋 산출
    let (ip_offset, ethertype_hint) = match datalink {
        1 => {
            if raw.len() < 14 {
                return None;
            }
            let et = u16::from_be_bytes([raw[12], raw[13]]);
            if et == 0x8100 && raw.len() >= 18 {
                (18, Some(u16::from_be_bytes([raw[16], raw[17]])))
            } else {
                (14, Some(et))
            }
        }
        0 | 108 => {
            if raw.len() < 4 {
                return None;
            }
            (4, None)
        }
        12 | 101 => (0, None),
        _ => {
            if raw.len() >= 14 {
                (14, Some(u16::from_be_bytes([raw[12], raw[13]])))
            } else {
                return None;
            }
        }
    };

    if raw.len() <= ip_offset {
        return None;
    }
    let l3 = &raw[ip_offset..];
    let version = l3[0] >> 4;

    // 2. IPv4 및 IPv6 분기
    let (src_ip, dst_ip, l4_proto, l4) = match (version, ethertype_hint) {
        (4, _) | (_, Some(0x0800)) => {
            if l3.len() < 20 {
                return None;
            }
            let ihl = ((l3[0] & 0x0f) as usize) * 4;
            if ihl < 20 || l3.len() < ihl {
                return None;
            }
            let proto = l3[9];
            let src = IpAddr::V4(Ipv4Addr::new(l3[12], l3[13], l3[14], l3[15]));
            let dst = IpAddr::V4(Ipv4Addr::new(l3[16], l3[17], l3[18], l3[19]));
            (src, dst, proto, &l3[ihl..])
        }
        (6, _) | (_, Some(0x86dd)) => {
            if l3.len() < 40 {
                return None;
            }
            let proto = l3[6];
            let src_bytes: [u8; 16] = l3[8..24].try_into().ok()?;
            let dst_bytes: [u8; 16] = l3[24..40].try_into().ok()?;
            let src = IpAddr::V6(Ipv6Addr::from(src_bytes));
            let dst = IpAddr::V6(Ipv6Addr::from(dst_bytes));
            (src, dst, proto, &l3[40..])
        }
        _ => return None,
    };

    // 3. TCP 헤더 및 페이로드 추출
    if l4_proto != 6 || l4.len() < 20 {
        return None;
    }
    let src_port = u16::from_be_bytes([l4[0], l4[1]]);
    let dst_port = u16::from_be_bytes([l4[2], l4[3]]);
    let data_offset = ((l4[12] >> 4) as usize) * 4;
    if data_offset < 20 || l4.len() < data_offset {
        return None;
    }
    let payload = &l4[data_offset..];

    // 4. 감시 대상 방향 판별: DB로 가는 요청만 SQL 디코딩, DB 응답(RX)은 길이만 집계
    // (응답 세그먼트마다 하던 문자열 변환·키워드 검색 제거, 비감시 트래픽은 채널에 넣지 않음)
    let is_outbound = watch.is_watched_endpoint(&dst_ip, dst_port);
    if !is_outbound && !watch.is_watched_endpoint(&src_ip, src_port) {
        return None;
    }
    let decoded_sql = if is_outbound && !payload.is_empty() {
        decode_wire_sql(dst_port, payload)
    } else {
        None
    };

    Some(WireFrame {
        src_ip,
        src_port,
        dst_ip,
        dst_port,
        payload_len: payload.len(),
        decoded_sql,
        captured_at: Instant::now(),
    })
}

// 3. DB 와이어 페이로드 분석 --------------------------------------------------
pub fn decode_wire_sql(dst_port: u16, payload: &[u8]) -> Option<DecodedSql> {
    let engine = DbEngine::from_port(dst_port);

    // 1. MySQL 패킷 분석 (3바이트 길이 + 1바이트 시퀀스 + 1바이트 커맨드)
    if matches!(engine, DbEngine::MySql) && payload.len() >= 5 {
        let pkt_len = (payload[0] as usize)
            | ((payload[1] as usize) << 8)
            | ((payload[2] as usize) << 16);
        let cmd = payload[4];
        if pkt_len >= 1 && pkt_len <= payload.len() + 65536 {
            let body = &payload[5..payload.len().min(4 + pkt_len)];
            match cmd {
                0x03 => {
                    if let Some(sql) = extract_clean_sql(body) {
                        return Some(analyze_sql_text(engine, &sql));
                    }
                }
                0x16 => {
                    if let Some(sql) = extract_clean_sql(body) {
                        let mut analyzed = analyze_sql_text(engine, &sql);
                        analyzed.op_kind = SqlOpKind::PrepareExec;
                        return Some(analyzed);
                    }
                }
                0x17 => {
                    return Some(DecodedSql {
                        engine,
                        op_kind: SqlOpKind::PrepareExec,
                        target_table: "PREPARED".to_string(),
                        fingerprint: "EXECUTE_STMT".to_string(),
                        preview: "[COM_STMT_EXECUTE] prepared statement execute".to_string(),
                        full_sql: "[COM_STMT_EXECUTE] binary execute, SQL text visible only at PREPARE"
                            .to_string(),
                        pattern_risk: RiskLevel::Safe,
                        pattern_note: None,
                    });
                }
                0x02 => {
                    if let Some(schema) = extract_clean_sql(body) {
                        return Some(DecodedSql {
                            engine,
                            op_kind: SqlOpKind::ShowMeta,
                            target_table: schema.clone(),
                            fingerprint: format!("USE {schema}"),
                            preview: format!("USE {schema}"),
                            full_sql: format!("USE {schema}"),
                            pattern_risk: RiskLevel::Safe,
                            pattern_note: None,
                        });
                    }
                }
                0x01 | 0x0e => {
                    return Some(DecodedSql {
                        engine,
                        op_kind: SqlOpKind::PingQuit,
                        target_table: "-".to_string(),
                        fingerprint: if cmd == 0x0e { "COM_PING" } else { "COM_QUIT" }.to_string(),
                        preview: if cmd == 0x0e { "[COM_PING]" } else { "[COM_QUIT]" }.to_string(),
                        full_sql: String::new(),
                        pattern_risk: RiskLevel::Safe,
                        pattern_note: None,
                    });
                }
                _ => {}
            }
        }
    }

    // 2. PostgreSQL Simple/Extended Query 분석 ('Q' / 'P')
    if matches!(engine, DbEngine::PostgreSql) && payload.len() >= 6 {
        let tag = payload[0];
        if tag == b'Q' || tag == b'P' {
            let body = &payload[5..];
            if let Some(sql) = extract_clean_sql(body) {
                return Some(analyze_sql_text(engine, &sql));
            }
        }
    }

    // 3. Redis RESP 명령은 SQL 아님 (값 안의 SELECT·UPDATE 단어 오인 방지, 트래픽만 집계)
    if matches!(engine, DbEngine::Redis) {
        let text = String::from_utf8_lossy(&payload[..payload.len().min(64)]);
        let command_token = if text.starts_with('*') {
            text.split("\r\n").nth(2).unwrap_or("")
        } else {
            text.split_whitespace().next().unwrap_or("")
        };
        let command = if !command_token.is_empty()
            && command_token.len() <= 24
            && command_token.chars().all(|c| c.is_ascii_alphabetic() || c == '.')
        {
            command_token.to_ascii_uppercase()
        } else {
            "DATA".to_string()
        };
        return Some(DecodedSql {
            engine,
            op_kind: SqlOpKind::EncryptedFlow,
            target_table: "-".to_string(),
            fingerprint: format!("REDIS::{command}"),
            preview: format!("[REDIS {command} {}B]", payload.len()),
            full_sql: String::new(),
            pattern_risk: RiskLevel::Safe,
            pattern_note: None,
        });
    }

    // 4. 일반 텍스트 SQL 폴백 스캔
    if let Some(sql) = scan_ascii_sql(payload) {
        return Some(analyze_sql_text(engine, &sql));
    }

    // 5. TLS 암호화 또는 바이너리 스트림 분류
    if payload.len() >= 16 {
        return Some(DecodedSql {
            engine,
            op_kind: SqlOpKind::EncryptedFlow,
            target_table: "-".to_string(),
            fingerprint: format!("{}::BINARY_OR_TLS_FLOW", engine.label()),
            preview: format!("[{} binary/TLS packet {}B]", engine.label(), payload.len()),
            full_sql: String::new(),
            pattern_risk: RiskLevel::Safe,
            pattern_note: None,
        });
    }

    None
}

// 4. SQL 원문 추출 및 정규화 --------------------------------------------------
fn extract_clean_sql(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let slice = &bytes[..end.min(2048)];
    if slice.is_empty() {
        return None;
    }
    let printable = slice
        .iter()
        .filter(|&&b| b.is_ascii_graphic() || b.is_ascii_whitespace() || b >= 0x80)
        .count();
    if printable * 10 < slice.len() * 8 {
        return None;
    }
    let text = String::from_utf8_lossy(slice).trim().to_string();
    if text.len() < 3 {
        return None;
    }
    Some(collapse_whitespace(&text))
}

fn scan_ascii_sql(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let collapsed = collapse_whitespace(&text);
    let upper = collapsed.to_ascii_uppercase();
    for kw in [
        "SELECT ",
        "INSERT ",
        "UPDATE ",
        "DELETE ",
        "SHOW ",
        "DESCRIBE ",
        "EXPLAIN ",
        "ALTER ",
        "CREATE ",
        "DROP ",
        "TRUNCATE ",
    ] {
        if let Some(idx) = upper.find(kw) {
            let sub = &collapsed[idx..];
            let clean: String = sub
                .chars()
                .take_while(|&c| !c.is_control())
                .take(2048)
                .collect();
            if clean.len() >= 6 {
                return Some(clean);
            }
        }
    }
    None
}

pub fn analyze_sql_text(engine: DbEngine, raw_sql: &str) -> DecodedSql {
    let sql = collapse_whitespace(raw_sql);
    let upper = sql.to_ascii_uppercase();
    let first_word = upper.split_whitespace().next().unwrap_or("");

    // 1. SQL 명령 분류
    let op_kind = match first_word {
        "SELECT" | "WITH" => SqlOpKind::Select,
        "INSERT" | "REPLACE" => SqlOpKind::Insert,
        "UPDATE" => SqlOpKind::Update,
        "DELETE" => SqlOpKind::Delete,
        "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "RENAME" => SqlOpKind::Ddl,
        "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "SET" | "USE" => SqlOpKind::ShowMeta,
        "BEGIN" | "START" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" => SqlOpKind::Transaction,
        "PREPARE" | "EXECUTE" => SqlOpKind::PrepareExec,
        _ => SqlOpKind::ShowMeta,
    };

    // 2. 대상 테이블 추출 및 핑거프린트 생성
    let target_table = extract_target_table(&upper, op_kind);
    let fingerprint = build_sql_fingerprint(&upper);

    // 3. 위험 쿼리 패턴 진단
    let (pattern_risk, pattern_note) = evaluate_sql_pattern(&upper, op_kind);
    // 표 셀용 미리보기와 하단 상세용 원문(첫 TCP 세그먼트 범위) 분리
    let preview = if sql.chars().count() > 140 {
        let head: String = sql.chars().take(140).collect();
        format!("{head}...")
    } else {
        sql.clone()
    };

    DecodedSql {
        engine,
        op_kind,
        target_table,
        fingerprint,
        preview,
        full_sql: sql,
        pattern_risk,
        pattern_note,
    }
}

// 5. 대상 테이블 및 핑거프린트 빌더 -------------------------------------------
fn extract_target_table(upper: &str, op_kind: SqlOpKind) -> String {
    let tokens: Vec<&str> = upper.split_whitespace().collect();
    let marker = match op_kind {
        SqlOpKind::Select | SqlOpKind::Delete => "FROM",
        SqlOpKind::Insert => "INTO",
        SqlOpKind::Update => "UPDATE",
        SqlOpKind::Ddl => "TABLE",
        _ => "",
    };

    if !marker.is_empty() {
        for idx in 0..tokens.len() {
            if tokens[idx] == marker && idx + 1 < tokens.len() {
                let candidate = tokens[idx + 1]
                    .trim_matches(|c: char| c == '`' || c == '"' || c == '\'' || c == '(' || c == ';');
                if !candidate.is_empty() && candidate != "SELECT" {
                    return candidate.to_string();
                }
            }
        }
    }
    "-".to_string()
}

pub fn build_sql_fingerprint(upper: &str) -> String {
    let mut out = String::with_capacity(upper.len().min(120));
    let mut chars = upper.chars().peekable();
    let mut in_single = false;

    while let Some(ch) = chars.next() {
        if ch == '\'' {
            if !in_single {
                out.push('?');
            }
            in_single = !in_single;
            continue;
        }
        if in_single {
            continue;
        }
        if ch.is_ascii_digit() {
            out.push('?');
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() || next == '.' {
                    chars.next();
                } else {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
        if out.len() >= 110 {
            break;
        }
    }
    collapse_whitespace(&out)
}

// 문장 하나만으로 DB에 피해를 줄 수 있는 경우만 위험, 조건 있는 쓰기·SELECT * 는 앱 정상 동작
// 조회량·반복 위험은 문장이 아니라 실측 RX·빈도로 risk.rs 에서 판정
fn evaluate_sql_pattern(upper: &str, op_kind: SqlOpKind) -> (RiskLevel, Option<RiskReason>) {
    // 1. 데이터·스키마 파괴
    if matches!(op_kind, SqlOpKind::Ddl)
        && (upper.starts_with("DROP ") || upper.starts_with("TRUNCATE "))
    {
        return (RiskLevel::Critical, Some(RiskReason::DestructiveDdl));
    }
    if matches!(op_kind, SqlOpKind::Delete | SqlOpKind::Update) && !upper.contains(" WHERE ") {
        return (RiskLevel::Critical, Some(RiskReason::UnconditionalWrite));
    }
    if upper.contains("SLEEP(") || upper.contains("BENCHMARK(") {
        return (RiskLevel::Critical, Some(RiskReason::BlockingFunction));
    }

    // 2. 잠금·재작성이 길어질 수 있는 스키마 변경
    if matches!(op_kind, SqlOpKind::Ddl) && upper.starts_with("ALTER ") {
        return (RiskLevel::Warning, Some(RiskReason::SchemaChange));
    }

    // 3. 조건·LIMIT 없는 전체 조회는 관찰만 (코드테이블·소형 테이블이 대부분)
    if matches!(op_kind, SqlOpKind::Select)
        && upper.contains(" FROM ")
        && !upper.contains(" WHERE ")
        && !upper.contains(" LIMIT ")
    {
        return (RiskLevel::Caution, Some(RiskReason::FullScan));
    }

    (RiskLevel::Safe, None)
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_select_star_is_safe_and_keeps_full_sql() {
        let res = analyze_sql_text(
            DbEngine::MySql,
            "SELECT * FROM tb_payment_history WHERE user_id = 4219",
        );
        assert_eq!(res.op_kind, SqlOpKind::Select);
        assert_eq!(res.target_table, "TB_PAYMENT_HISTORY");
        assert_eq!(res.pattern_risk, RiskLevel::Safe);
        assert!(res.fingerprint.contains("USER_ID = ?"));
        assert_eq!(res.full_sql, "SELECT * FROM tb_payment_history WHERE user_id = 4219");

        let scan = analyze_sql_text(DbEngine::MySql, "SELECT * FROM tb_code");
        assert_eq!(scan.pattern_risk, RiskLevel::Caution);

        let long_sql = format!("SELECT {} FROM tb_order WHERE id = 1", "col_a, ".repeat(60));
        let long = analyze_sql_text(DbEngine::MySql, &long_sql);
        assert!(long.preview.ends_with("..."));
        assert!(long.full_sql.ends_with("WHERE id = 1"));
    }

    #[test]
    fn detects_unbounded_delete_as_critical() {
        let res = analyze_sql_text(DbEngine::MySql, "DELETE FROM tb_outbox_event");
        assert_eq!(res.op_kind, SqlOpKind::Delete);
        assert_eq!(res.pattern_risk, RiskLevel::Critical);
    }

    #[test]
    fn conditional_write_is_safe_and_alter_is_warning() {
        let update = analyze_sql_text(
            DbEngine::MySql,
            "UPDATE tb_user SET last_login_dt = NOW() WHERE user_id = 'u1'",
        );
        let insert = analyze_sql_text(
            DbEngine::MySql,
            "INSERT INTO tb_access_log (user_id, menu_id) VALUES ('u1', 'm1')",
        );
        let alter = analyze_sql_text(
            DbEngine::MySql,
            "ALTER TABLE tb_user ADD COLUMN memo VARCHAR(10)",
        );
        let truncate = analyze_sql_text(DbEngine::MySql, "TRUNCATE TABLE tb_user");
        assert_eq!(update.pattern_risk, RiskLevel::Safe);
        assert_eq!(insert.pattern_risk, RiskLevel::Safe);
        assert_eq!(alter.pattern_risk, RiskLevel::Warning);
        assert_eq!(truncate.pattern_risk, RiskLevel::Critical);
    }

    #[test]
    fn packet_decodes_sql_only_toward_db_and_drops_unwatched_traffic() {
        // 원시 IPv4(DLT_RAW 12) + TCP 헤더 20바이트씩, 데이터 오프셋 5워드
        let build_packet = |src: [u8; 4], sport: u16, dst: [u8; 4], dport: u16, payload: &[u8]| {
            let mut raw = vec![0u8; 40];
            raw[0] = 0x45;
            raw[9] = 6;
            raw[12..16].copy_from_slice(&src);
            raw[16..20].copy_from_slice(&dst);
            raw[20..22].copy_from_slice(&sport.to_be_bytes());
            raw[22..24].copy_from_slice(&dport.to_be_bytes());
            raw[32] = 0x50;
            raw.extend_from_slice(payload);
            raw
        };
        let cfg = MonitorConfig::default();
        let client = [192, 0, 2, 25];
        let db = [192, 0, 2, 10];
        let sql = b"SELECT id FROM tb_order WHERE id = 1";
        let mut query = vec![(sql.len() + 1) as u8, 0, 0, 0, 0x03];
        query.extend_from_slice(sql);

        let outbound = decode_packet(&build_packet(client, 55101, db, 3306, &query), 12, &cfg)
            .expect("outbound frame");
        assert_eq!(outbound.decoded_sql.expect("sql").op_kind, SqlOpKind::Select);

        let reply = b"....SELECT text inside result row....";
        let inbound = decode_packet(&build_packet(db, 3306, client, 55101, reply), 12, &cfg)
            .expect("inbound frame");
        assert!(inbound.decoded_sql.is_none());
        assert_eq!(inbound.payload_len, reply.len());

        assert!(decode_packet(&build_packet(client, 55101, db, 8080, sql), 12, &cfg).is_none());
    }

    #[test]
    fn redis_value_words_are_not_decoded_as_sql() {
        let set_cmd = b"*3\r\n$3\r\nSET\r\n$7\r\nmenu:01\r\n$27\r\n{\"title\":\"select update\"}\r\n";
        let select_db = b"*2\r\n$6\r\nSELECT\r\n$1\r\n1\r\n";
        let decoded_set = decode_wire_sql(6379, set_cmd).expect("redis set");
        let decoded_select = decode_wire_sql(6379, select_db).expect("redis select");

        assert_eq!(decoded_set.op_kind, SqlOpKind::EncryptedFlow);
        assert_eq!(decoded_set.fingerprint, "REDIS::SET");
        assert_eq!(decoded_select.op_kind, SqlOpKind::EncryptedFlow);
        assert_eq!(decoded_select.fingerprint, "REDIS::SELECT");
    }
}
