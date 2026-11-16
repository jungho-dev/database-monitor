//! build.rs
//! Windows 실행 파일에 앱 아이콘·버전 정보(VERSIONINFO)·asInvoker manifest 리소스를 windres 로 컴파일해 링크.
//! rc 스크립트·manifest 는 빌드마다 OUT_DIR 에 만들고(값은 CARGO_PKG_*), 아이콘만 assets/icon/database-monitor.ico 를 참조.
//! gnullvm 타겟은 libunwind.dll 을 동적 링크하므로 target/<profile> 옆에 복사해 PATH 없이 실행되게 함.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const DLL_NAME: &str = "libunwind.dll";
const ICON_PATH: &str = "assets/icon/database-monitor.ico";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed={ICON_PATH}");
    println!("cargo:rerun-if-env-changed=WINDRES");
    println!("cargo:rerun-if-env-changed=RUSTC_LINKER");

    let target = env::var("TARGET").unwrap_or_default();
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_resources();
    }
    if target.contains("gnullvm") {
        copy_libunwind_dll(&target);
    }
}

// 1. libunwind.dll 탐색 -------------------------------------------------------
fn find_libunwind_dll(target: &str) -> Option<PathBuf> {
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let output = Command::new(rustc)
        .args(["--print", "sysroot"])
        .output()
        .ok()?;
    let sysroot = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());

    // host 툴체인 bin (host == target 인 일반적인 경우)
    let host_bin = sysroot.join("bin").join(DLL_NAME);
    if host_bin.is_file() {
        return Some(host_bin);
    }

    // 크로스 컴파일 시 타겟 전용 rustlib bin
    let target_bin = sysroot
        .join("lib")
        .join("rustlib")
        .join(target)
        .join("bin")
        .join(DLL_NAME);
    target_bin.is_file().then_some(target_bin)
}

// 2. libunwind.dll 복사 -------------------------------------------------------
// OUT_DIR = <target_dir>/<profile>/build/<pkg>-<hash>/out 이므로 3단계 상위가 exe 폴더.
fn copy_libunwind_dll(target: &str) {
    let Some(dll_src) = find_libunwind_dll(target) else {
        println!(
            "cargo:warning={DLL_NAME} 를 찾지 못함 (target={target}), 실행 시 로드 실패할 수 있음"
        );
        return;
    };
    let Some(profile_dir) = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .and_then(|out_dir| out_dir.ancestors().nth(3).map(Path::to_path_buf))
    else {
        println!("cargo:warning=OUT_DIR 에서 target 디렉터리 경로를 계산하지 못함");
        return;
    };

    // 실행 중인 바이너리가 DLL 을 잠그므로(os error 32) 내용이 같으면 복사하지 않음
    let dest = profile_dir.join(DLL_NAME);
    let src_bytes = std::fs::read(&dll_src).ok();
    if src_bytes.is_some() && src_bytes == std::fs::read(&dest).ok() {
        return;
    }
    if let Err(err) = std::fs::copy(&dll_src, &dest) {
        println!(
            "cargo:warning={DLL_NAME} 복사 실패 ({err}): {}",
            dest.display()
        );
    }
}

// 3. VERSIONINFO + manifest + 아이콘 리소스 삽입 ------------------------------
// 메타데이터·manifest 없는 서명 없는 exe 는 백신 ML 휴리스틱 감점 요인이라 링크 시점에 삽입.
// 리소스 도구가 없어도 빌드는 계속 (리소스만 빠짐).
fn embed_windows_resources() {
    let Some(out_dir) = env::var_os("OUT_DIR").map(PathBuf::from) else {
        println!("cargo:warning=OUT_DIR 없음, 리소스 삽입 생략");
        return;
    };
    let name = env::var("CARGO_PKG_NAME").unwrap_or_default();
    let description = env::var("CARGO_PKG_DESCRIPTION").unwrap_or_default();
    let license = env::var("CARGO_PKG_LICENSE").unwrap_or_default();
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let numeric_version = ["MAJOR", "MINOR", "PATCH"]
        .map(|part| env::var(format!("CARGO_PKG_VERSION_{part}")).unwrap_or_else(|_| "0".into()))
        .join(",");
    let dotted_version = numeric_version.replace(',', ".");

    // 1. manifest 작성 (설명은 XML 특수 문자를 이스케이프)
    let manifest_path = out_dir.join("app.manifest");
    let manifest_description = description
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let manifest = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="{name}" version="{dotted_version}.0" processorArchitecture="*"/>
  <description>{manifest_description}</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
</assembly>
"#
    );

    // 2. rc 스크립트 작성
    // 아이콘은 CARGO_MANIFEST_DIR 기준 절대경로(슬래시)로 참조한다. 파일이 없으면 경고만 남기고 생략.
    let description_rc = description.replace('"', "\"\"");
    let manifest_ref = manifest_path.display().to_string().replace('\\', "/");
    let icon_path = env::var_os("CARGO_MANIFEST_DIR")
        .map(|dir| PathBuf::from(dir).join(ICON_PATH))
        .filter(|path| path.is_file());
    let icon_line = match icon_path {
        Some(path) => format!(
            "1 ICON \"{}\"\n",
            path.display().to_string().replace('\\', "/")
        ),
        None => {
            println!("cargo:warning={ICON_PATH} 없음, 아이콘 없이 빌드");
            String::new()
        }
    };
    let rc_path = out_dir.join("app.rc");
    let rc_script = format!(
        r#"#pragma code_page(65001)
{icon_line}1 24 "{manifest_ref}"
1 VERSIONINFO
FILEVERSION {numeric_version},0
PRODUCTVERSION {numeric_version},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "{name} contributors"
      VALUE "FileDescription", "{description_rc}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{name}"
      VALUE "LegalCopyright", "Licensed under {license}"
      VALUE "OriginalFilename", "{name}.exe"
      VALUE "ProductName", "{name}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    if let Err(err) =
        std::fs::write(&manifest_path, manifest).and_then(|()| std::fs::write(&rc_path, rc_script))
    {
        println!("cargo:warning=리소스 원본 쓰기 실패 ({err}), 리소스 삽입 생략");
        return;
    }

    // 3. windres 후보 (WINDRES 지정값, 링커 폴더, PATH 순)
    // llvm-windres 와 접두사 없는 windres 는 호스트(x64) COFF 를 기본 출력하므로
    // 크로스 타겟(aarch64 등)에서는 --target 트리플로 머신 타입을 지정해야 한다.
    let (mingw_arch, coff_machine): (String, u16) = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => ("i686".to_string(), 0x014c),
        Ok("arm") => ("armv7".to_string(), 0x01c4),
        Ok("aarch64") => ("aarch64".to_string(), 0xaa64),
        Ok(arch) => (arch.to_string(), 0x8664),
        Err(_) => ("x86_64".to_string(), 0x8664),
    };
    let llvm_target = format!("--target={mingw_arch}-w64-mingw32");
    let tool_names = [
        (format!("{mingw_arch}-w64-mingw32-windres"), None),
        ("llvm-windres".to_string(), Some(llvm_target.as_str())),
        ("windres".to_string(), None),
    ];
    let mut candidates: Vec<(PathBuf, Option<&str>)> = env::var_os("WINDRES")
        .map(|windres| (PathBuf::from(windres), None))
        .into_iter()
        .collect();
    let linker_dir = env::var_os("RUSTC_LINKER")
        .map(PathBuf::from)
        .and_then(|linker| linker.parent().map(Path::to_path_buf));
    if let Some(dir) = &linker_dir {
        for (tool, target_arg) in &tool_names {
            let exe_path = dir.join(format!("{tool}.exe"));
            if exe_path.is_file() {
                candidates.push((exe_path, *target_arg));
            }
        }
    }
    candidates.extend(
        tool_names
            .iter()
            .map(|(tool, target_arg)| (PathBuf::from(tool), *target_arg)),
    );

    // 4. 리소스 컴파일 및 링크
    // COFF 헤더 첫 2바이트(머신 타입, LE)가 타겟과 다르면 링커가 LNK1112로 죽으므로 버리고 다음 후보로 넘어간다.
    let object_path = out_dir.join("app-resource.o");
    for (windres, target_arg) in &candidates {
        let _ = std::fs::remove_file(&object_path);
        let mut command = Command::new(windres);
        if let Some(target_arg) = target_arg {
            command.arg(target_arg);
        }
        let status = command
            .arg("-O")
            .arg("coff")
            .arg("-i")
            .arg(&rc_path)
            .arg("-o")
            .arg(&object_path)
            .status();
        if !matches!(status, Ok(exit) if exit.success()) {
            continue;
        }
        let object_machine = std::fs::read(&object_path)
            .ok()
            .and_then(|bytes| Some(u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?])));
        if object_machine == Some(coff_machine) {
            println!("cargo:rustc-link-arg-bins={}", object_path.display());
            return;
        }
        println!(
            "cargo:warning={} 출력 머신 타입 불일치 (expected={coff_machine:#06x}, actual={object_machine:#06x?}), 다음 후보 시도",
            windres.display()
        );
    }
    let _ = std::fs::remove_file(&object_path);
    println!("cargo:warning=windres 실행 실패, 아이콘·VERSIONINFO·manifest 없이 빌드");
}
