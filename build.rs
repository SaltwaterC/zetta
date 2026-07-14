use std::{env, path::Path, process::Command};

const CONPTY_PACKAGE_URL: &str = "https://github.com/microsoft/terminal/releases/download/v1.24.10621.0/Microsoft.Windows.Console.ConPTY.1.24.260303001.nupkg";
const CONPTY_PACKAGE_ID: &str = "1.24.260303001";
const CONPTY_PACKAGE_SHA256: &str =
    "2C57CB7DA7E19FA06C86487C8D9B5C307D65695429FA15A854BF5F3CDDCA9E1D";

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon = "assets/icons/zetta-terminal-icon.ico";
    let resource = "resources/windows/zetta.rc";

    println!("cargo:rerun-if-changed={icon}");
    println!("cargo:rerun-if-changed={resource}");

    embed_resource::compile(resource, embed_resource::NONE)
        .manifest_required()
        .unwrap();

    stage_conpty_runtime();
}

fn stage_conpty_runtime() {
    let out_dir = env::var("OUT_DIR").expect("Cargo did not provide OUT_DIR");
    let out_dir = Path::new(&out_dir);
    let target_dir = out_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("could not locate the Cargo target directory");
    let conpty_target = target_dir.join("conpty.dll");
    let open_console_target = target_dir.join("OpenConsole.exe");
    let architecture =
        env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo did not provide target arch");
    let (runtime_arch, console_arch) = match architecture.as_str() {
        "x86_64" => ("win-x64", "x64"),
        "aarch64" => ("win-arm64", "arm64"),
        other => panic!("unsupported Windows architecture for ConPTY: {other}"),
    };

    let target_root = target_dir
        .parent()
        .expect("could not locate the Cargo target root");
    let cache_dir = target_root
        .join("zetta-conpty")
        .join(CONPTY_PACKAGE_ID)
        .join(&architecture);
    let conpty_source = cache_dir.join("conpty.dll");
    let open_console_source = cache_dir.join("OpenConsole.exe");

    if !conpty_source.is_file() || !open_console_source.is_file() {
        std::fs::create_dir_all(&cache_dir).expect("failed to create the ConPTY cache");
        let archive = out_dir.join("conpty.nupkg.zip");
        let extracted = out_dir.join("conpty");
        run_powershell(&format!(
            "$ProgressPreference = 'SilentlyContinue'; Invoke-WebRequest -Uri '{}' -OutFile '{}'",
            CONPTY_PACKAGE_URL,
            powershell_path(&archive)
        ));
        verify_sha256(&archive, CONPTY_PACKAGE_SHA256);
        run_powershell(&format!(
            "$ProgressPreference = 'SilentlyContinue'; Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            powershell_path(&archive),
            powershell_path(&extracted)
        ));

        copy_runtime(
            &extracted
                .join("runtimes")
                .join(runtime_arch)
                .join("native")
                .join("conpty.dll"),
            &conpty_source,
        );
        copy_runtime(
            &extracted
                .join("build")
                .join("native")
                .join("runtimes")
                .join(console_arch)
                .join("OpenConsole.exe"),
            &open_console_source,
        );
    }

    copy_runtime(&conpty_source, &conpty_target);
    copy_runtime(&open_console_source, &open_console_target);
}

fn copy_runtime(source: &Path, target: &Path) {
    std::fs::copy(source, target).unwrap_or_else(|error| {
        panic!(
            "failed to stage {} as {}: {error}",
            source.display(),
            target.display()
        )
    });
}

fn run_powershell(script: &str) {
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .status()
        .expect("failed to start PowerShell while staging ConPTY");
    assert!(status.success(), "PowerShell failed while staging ConPTY");
}

fn verify_sha256(path: &Path, expected: &str) {
    let output = Command::new("certutil")
        .arg("-hashfile")
        .arg(path)
        .arg("SHA256")
        .output()
        .expect("failed to start certutil while verifying ConPTY");
    assert!(
        output.status.success(),
        "certutil failed while verifying ConPTY"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let actual = stdout
        .lines()
        .map(str::trim)
        .find(|line| line.len() == 64 && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .expect("certutil did not return a SHA256 hash");
    assert!(
        actual.eq_ignore_ascii_case(expected),
        "ConPTY package checksum mismatch: expected {expected}, got {actual}"
    );
}

fn powershell_path(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}
