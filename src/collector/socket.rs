//! socket.rs
//! Windows TCP 소켓 테이블(GetExtendedTcpTable) 및 프로세스 메타데이터 패시브 수집기.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
#[cfg(windows)]
use std::net::{Ipv4Addr, Ipv6Addr};
use crate::collector::process::{
    ProcEntry, ProcMetadata, classify_with_ancestry, query_command_line, snapshot_process_table,
};
use crate::app::config::MonitorConfig;
use crate::app::model::{ActorKind, TcpConnState};

// 1. 수집된 원시 소켓 레코드 --------------------------------------------------
#[derive(Debug, Clone)]
pub struct RawDbSocket {
    pub local_addr: IpAddr,
    pub local_port: u16,
    pub remote_addr: IpAddr,
    pub remote_port: u16,
    pub state: TcpConnState,
    pub pid: u32,
}

// 2. 프로세스 캐시 및 소켓 스캐너 ---------------------------------------------
type FlowTuple = (IpAddr, u16, IpAddr, u16);

pub struct SocketScanner {
    proc_cache: HashMap<u32, ProcMetadata>,
    flow_owners: HashMap<FlowTuple, u32>,
}

impl Default for SocketScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl SocketScanner {
    pub fn new() -> Self {
        Self {
            proc_cache: HashMap::new(),
            flow_owners: HashMap::new(),
        }
    }

    pub fn scan(&mut self, cfg: &MonitorConfig) -> (Vec<RawDbSocket>, HashMap<u32, ProcMetadata>) {
        // 1. TCP 테이블 조회 및 TIME_WAIT 소유 PID 복원 (PID 0 행이 있을 때만 테이블 조회)
        let mut sockets = collect_watched_sockets(cfg);
        let mut live_table = sockets
            .iter()
            .any(|s| s.pid == 0)
            .then(snapshot_process_table);
        self.restore_flow_owners(&mut sockets, live_table.as_ref().unwrap_or(&HashMap::new()));

        // 2. 종료 PID 캐시 제거 (PID 재사용 오인 방지), 소켓 수 x 캐시 수 순회 대신 집합 조회
        let live_pids: HashSet<u32> = sockets.iter().map(|s| s.pid).collect();
        self.proc_cache.retain(|pid, _| live_pids.contains(pid));
        if sockets.is_empty() {
            return (sockets, HashMap::new());
        }

        // 3. 신규 PID 메타데이터 보강
        let mut missing_pids: Vec<u32> = live_pids
            .iter()
            .copied()
            .filter(|&pid| pid > 0 && !self.proc_cache.contains_key(&pid))
            .collect();
        missing_pids.sort_unstable();

        if !missing_pids.is_empty() {
            let table = &*live_table.get_or_insert_with(snapshot_process_table);
            for pid in missing_pids {
                let name = table
                    .get(&pid)
                    .map(|entry| entry.name.clone())
                    .unwrap_or_else(|| format!("pid-{pid}"));
                let cmdline = query_command_line(pid).unwrap_or_else(|| name.clone());
                let actor_kind = classify_with_ancestry(pid, &name, &cmdline, table);
                self.proc_cache.insert(
                    pid,
                    ProcMetadata {
                        pid,
                        name,
                        cmdline,
                        actor_kind,
                    },
                );
            }
        }

        // 4. 활성 PID 메타데이터 맵 반환 (PID 당 1회 복제, TIME_WAIT 수백 개여도 커맨드라인 복제 1회)
        let mut active_meta = HashMap::with_capacity(live_pids.len());
        for pid in live_pids {
            let meta = self.proc_cache.get(&pid).cloned().unwrap_or_else(|| ProcMetadata {
                pid,
                name: format!("pid-{pid}"),
                cmdline: String::new(),
                actor_kind: ActorKind::SystemOther,
            });
            active_meta.insert(pid, meta);
        }

        (sockets, active_meta)
    }

    // TIME_WAIT 소켓은 OS 테이블에서 PID 0, 직전 스캔의 같은 4-튜플 소유자로 복원
    // 소유자가 종료됐거나 PID 가 다른 프로세스로 재사용되면 복원하지 않음
    pub fn restore_flow_owners(
        &mut self,
        sockets: &mut [RawDbSocket],
        live_table: &HashMap<u32, ProcEntry>,
    ) {
        let mut seen = HashMap::with_capacity(sockets.len());
        for sock in sockets.iter_mut() {
            let flow = (sock.local_addr, sock.local_port, sock.remote_addr, sock.remote_port);
            if sock.pid == 0
                && let Some(&owner) = self.flow_owners.get(&flow)
            {
                let cached_name = self.proc_cache.get(&owner).map(|m| m.name.as_str());
                let owner_alive = live_table.get(&owner).is_some_and(|entry| {
                    cached_name.is_none_or(|name| entry.name.eq_ignore_ascii_case(name))
                });
                if owner_alive {
                    sock.pid = owner;
                }
            }
            if sock.pid > 0 {
                seen.insert(flow, sock.pid);
            }
        }
        self.flow_owners = seen;
    }
}

// 3. iphlpapi FFI 선언 --------------------------------------------------------
#[cfg(windows)]
mod ffi {
    use std::ffi::c_void;

    pub const AF_INET: u32 = 2;
    pub const AF_INET6: u32 = 23;
    pub const TCP_TABLE_OWNER_PID_ALL: u32 = 5;
    pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        pub fn GetExtendedTcpTable(
            table: *mut c_void,
            size: *mut u32,
            order: i32,
            family: u32,
            table_class: u32,
            reserved: u32,
        ) -> u32;
    }
}

// 4. TCP 테이블 조회 및 감시 대상 필터 ----------------------------------------
#[cfg(windows)]
pub fn collect_watched_sockets(cfg: &MonitorConfig) -> Vec<RawDbSocket> {
    let mut results = Vec::new();

    // MIB_TCPROW_OWNER_PID 6워드, MIB_TCP6ROW_OWNER_PID 14워드
    for (family, row_words) in [(ffi::AF_INET, 6usize), (ffi::AF_INET6, 14usize)] {
        // 1. 버퍼 크기 협상 (연결 증가 대비 재시도)
        let mut words: Vec<u32> = Vec::new();
        let mut size_bytes = 0u32;
        let mut loaded = false;
        for _ in 0..4 {
            let table_ptr = if words.is_empty() {
                std::ptr::null_mut()
            } else {
                words.as_mut_ptr().cast()
            };
            // SAFETY: size_bytes 는 words 실제 바이트 길이 이하
            let ret = unsafe {
                ffi::GetExtendedTcpTable(
                    table_ptr,
                    &mut size_bytes,
                    0,
                    family,
                    ffi::TCP_TABLE_OWNER_PID_ALL,
                    0,
                )
            };
            if ret == 0 {
                loaded = true;
                break;
            }
            if ret != ffi::ERROR_INSUFFICIENT_BUFFER {
                break;
            }
            words = vec![0; (size_bytes as usize).div_ceil(4) + 64];
            size_bytes = (words.len() * 4) as u32;
        }
        if !loaded || words.is_empty() {
            continue;
        }

        // 2. 행 파싱 (주소는 메모리 순서 바이트, 포트는 네트워크 바이트 순서)
        let row_count = words[0] as usize;
        let max_rows = (words.len() - 1) / row_words;
        for row in words[1..].chunks_exact(row_words).take(row_count.min(max_rows)) {
            let (local_addr, local_port, remote_addr, remote_port, state_code, pid) =
                if family == ffi::AF_INET {
                    (
                        IpAddr::V4(Ipv4Addr::from(row[1].to_ne_bytes())),
                        u16::from_be(row[2] as u16),
                        IpAddr::V4(Ipv4Addr::from(row[3].to_ne_bytes())),
                        u16::from_be(row[4] as u16),
                        row[0],
                        row[5],
                    )
                } else {
                    let mut local_bytes = [0u8; 16];
                    let mut remote_bytes = [0u8; 16];
                    for idx in 0..4 {
                        local_bytes[idx * 4..idx * 4 + 4].copy_from_slice(&row[idx].to_ne_bytes());
                        remote_bytes[idx * 4..idx * 4 + 4]
                            .copy_from_slice(&row[6 + idx].to_ne_bytes());
                    }
                    (
                        Ipv6Addr::from(local_bytes).to_canonical(),
                        u16::from_be(row[5] as u16),
                        Ipv6Addr::from(remote_bytes).to_canonical(),
                        u16::from_be(row[11] as u16),
                        row[12],
                        row[13],
                    )
                };

            if remote_port == 0 || remote_addr.is_unspecified() {
                continue;
            }
            if !cfg.is_watched_endpoint(&remote_addr, remote_port) {
                continue;
            }
            results.push(RawDbSocket {
                local_addr,
                local_port,
                remote_addr,
                remote_port,
                state: TcpConnState::from_mib(state_code),
                pid,
            });
        }
    }

    results
}

#[cfg(not(windows))]
pub fn collect_watched_sockets(_cfg: &MonitorConfig) -> Vec<RawDbSocket> {
    Vec::new()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn native_tcp_table_reports_own_ipv4_and_ipv6_connections() {
        let listener_v4 = TcpListener::bind("127.0.0.1:0").expect("bind v4");
        let port_v4 = listener_v4.local_addr().unwrap().port();
        let _client_v4 = TcpStream::connect(("127.0.0.1", port_v4)).expect("connect v4");
        let listener_v6 = TcpListener::bind("[::1]:0").ok();
        let port_v6 = listener_v6.as_ref().map(|l| l.local_addr().unwrap().port());
        let _client_v6 = port_v6.and_then(|port| TcpStream::connect(("::1", port)).ok());

        let mut cfg = MonitorConfig {
            watched_ports: vec![port_v4],
            ..MonitorConfig::default()
        };
        cfg.watched_ports.extend(port_v6);
        let rows = collect_watched_sockets(&cfg);

        let own_v4 = rows
            .iter()
            .find(|r| r.remote_port == port_v4 && r.remote_addr.is_ipv4())
            .expect("own v4 connection");
        assert_eq!(own_v4.pid, std::process::id());
        assert_eq!(own_v4.state, TcpConnState::Established);
        assert!(crate::app::config::is_local_endpoint(&own_v4.local_addr, &own_v4.remote_addr));

        if let (Some(port), Some(_)) = (port_v6, &_client_v6) {
            assert!(rows.iter().any(|r| r.remote_port == port && r.remote_addr.is_ipv6()));
        }
        assert!(rows.iter().all(|r| cfg.watched_ports.contains(&r.remote_port)));
    }

    #[test]
    fn time_wait_row_inherits_previous_owner_pid() {
        let local_ip: IpAddr = "192.0.2.25".parse().unwrap();
        let db_ip: IpAddr = "192.0.2.10".parse().unwrap();
        let row = |state, pid| RawDbSocket {
            local_addr: local_ip,
            local_port: 50_001,
            remote_addr: db_ip,
            remote_port: 3306,
            state,
            pid,
        };
        let mut scanner = SocketScanner::new();
        let mut live_table = HashMap::new();
        live_table.insert(
            4242,
            ProcEntry {
                name: "java.exe".to_string(),
                parent_pid: 1,
            },
        );

        let mut first = vec![row(TcpConnState::Established, 4242)];
        scanner.restore_flow_owners(&mut first, &live_table);
        let mut second = vec![row(TcpConnState::TimeWait, 0)];
        scanner.restore_flow_owners(&mut second, &live_table);
        assert_eq!(second[0].pid, 4242);

        // 사라진 흐름은 소유자 기억에서 제거
        scanner.restore_flow_owners(&mut [], &live_table);
        let mut third = vec![row(TcpConnState::TimeWait, 0)];
        scanner.restore_flow_owners(&mut third, &live_table);
        assert_eq!(third[0].pid, 0);
    }

    #[test]
    fn time_wait_row_drops_exited_or_reused_owner() {
        let local_ip: IpAddr = "192.0.2.25".parse().unwrap();
        let db_ip: IpAddr = "192.0.2.10".parse().unwrap();
        let row = |state, pid| RawDbSocket {
            local_addr: local_ip,
            local_port: 50_002,
            remote_addr: db_ip,
            remote_port: 6379,
            state,
            pid,
        };
        let mut scanner = SocketScanner::new();
        scanner.proc_cache.insert(
            9108,
            ProcMetadata {
                pid: 9108,
                name: "java.exe".to_string(),
                cmdline: String::new(),
                actor_kind: ActorKind::JvmSpringApp,
            },
        );
        let mut live_table = HashMap::new();
        live_table.insert(
            9108,
            ProcEntry {
                name: "java.exe".to_string(),
                parent_pid: 1,
            },
        );
        scanner.restore_flow_owners(&mut [row(TcpConnState::Established, 9108)], &live_table);

        // 소유 PID 가 다른 이름으로 재사용되면 복원 안 함
        live_table.insert(
            9108,
            ProcEntry {
                name: "notepad.exe".to_string(),
                parent_pid: 1,
            },
        );
        let mut reused = vec![row(TcpConnState::TimeWait, 0)];
        scanner.restore_flow_owners(&mut reused, &live_table);
        assert_eq!(reused[0].pid, 0);

        // 소유자 종료 후 남은 TIME_WAIT 은 PID 0 유지
        scanner.restore_flow_owners(&mut [row(TcpConnState::Established, 9108)], &live_table);
        let mut exited = vec![row(TcpConnState::TimeWait, 0)];
        scanner.restore_flow_owners(&mut exited, &HashMap::new());
        assert_eq!(exited[0].pid, 0);
    }
}
