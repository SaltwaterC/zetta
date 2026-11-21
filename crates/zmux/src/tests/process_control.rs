#![cfg(unix)]

use std::{
    fs,
    io::BufReader,
    os::unix::{fs::PermissionsExt as _, net::UnixListener},
    path::Path,
    thread,
};

use super::*;

fn private_session_directory() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("sessions");
    crate::private_fs::create_private_dir(&directory).unwrap();
    (root, directory)
}

/// Publishes an endpoint for this test process, which is running, naming
/// `socket_path`.
fn publish(path: &Path, socket_path: &Path) {
    publish_version(path, socket_path, CONTROL_VERSION);
}

fn publish_version(path: &Path, socket_path: &Path, version: u32) {
    let endpoint = serde_json::json!({
        "version": version,
        "process_id": std::process::id(),
        "socket_path": socket_path,
        "token": "token",
    });
    crate::private_fs::write_private_file(path, endpoint.to_string().as_bytes()).unwrap();
}

fn own_endpoint_path(directory: &Path) -> PathBuf {
    directory.join(format!("control-{}.json", std::process::id()))
}

#[test]
fn a_running_windows_own_endpoint_is_found() {
    let (_root, directory) = private_session_directory();
    let path = own_endpoint_path(&directory);
    publish(&path, &path.with_extension("sock"));

    let endpoints = running_control_endpoints(&directory).unwrap();
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints[0].socket_path, path.with_extension("sock"));
}

#[test]
fn a_request_carries_its_fields_beside_the_token_and_command() {
    let (_root, directory) = private_session_directory();
    let path = own_endpoint_path(&directory);
    let listener = UnixListener::bind(path.with_extension("sock")).unwrap();
    publish(&path, &path.with_extension("sock"));
    let endpoint = read_control_endpoint(&path, std::process::id()).unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&mut stream).read_line(&mut line).unwrap();
        stream.write_all(b"{\"status\":\"ok\"}\n").unwrap();
        serde_json::from_str::<serde_json::Value>(&line).unwrap()
    });

    #[derive(Serialize)]
    struct Fields {
        attention_id: u64,
        worktree_name: Option<String>,
    }
    let response: ControlResponse = request(
        &endpoint,
        "set_worktree_name",
        &Fields {
            attention_id: 7,
            worktree_name: None,
        },
        Duration::from_secs(5),
    )
    .unwrap();

    assert_eq!(response.status, "ok");
    assert_eq!(
        server.join().unwrap(),
        serde_json::json!({
            "token": "token",
            "command": "set_worktree_name",
            "attention_id": 7,
            "worktree_name": null,
        })
    );
}

#[test]
fn reconnect_discovers_a_window_with_tab_targeted_icon_support() {
    let (_root, directory) = private_session_directory();
    let path = own_endpoint_path(&directory);
    // The GUI and standalone reconnect client must use the same marker,
    // including when other process-control commands change.
    publish_version(&path, &path.with_extension("sock"), CONTROL_VERSION);

    assert_eq!(running_control_endpoints(&directory).unwrap().len(), 1);

    publish_version(&path, &path.with_extension("sock"), CONTROL_VERSION + 1);
    assert!(running_control_endpoints(&directory).unwrap().is_empty());
}

#[test]
fn an_endpoint_naming_a_socket_other_than_its_own_is_not_used() {
    let (root, directory) = private_session_directory();
    publish(
        &own_endpoint_path(&directory),
        &root.path().join("elsewhere.sock"),
    );

    assert!(running_control_endpoints(&directory).unwrap().is_empty());
    let Err(error) = read_control_endpoint(&own_endpoint_path(&directory), std::process::id())
    else {
        panic!("an endpoint naming another socket must be refused");
    };
    assert!(format!("{error:#}").contains("not its own"), "{error:#}");
}

#[test]
fn a_symlinked_endpoint_file_is_not_trusted() {
    let (root, directory) = private_session_directory();
    let planted = root.path().join("planted.json");
    let path = own_endpoint_path(&directory);
    publish(&planted, &path.with_extension("sock"));
    std::os::unix::fs::symlink(&planted, &path).unwrap();

    assert!(running_control_endpoints(&directory).unwrap().is_empty());
}

#[test]
fn an_endpoint_published_under_another_processs_name_is_not_used() {
    let (_root, directory) = private_session_directory();
    let path = directory.join(format!("control-{}.json", std::process::id() + 1));
    publish(&path, &path.with_extension("sock"));

    assert!(running_control_endpoints(&directory).unwrap().is_empty());
}

#[test]
fn endpoints_are_not_read_from_a_directory_others_can_write() {
    let (_root, directory) = private_session_directory();
    let path = own_endpoint_path(&directory);
    publish(&path, &path.with_extension("sock"));
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();

    assert!(running_control_endpoints(&directory).is_err());
}
