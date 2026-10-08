use super::*;

#[test]
fn config_path_identity_is_absolute_and_lexically_normalized() {
    let relative = config_path_identity(Path::new("./config/../config.json"));
    let absolute = config_path_identity(&std::env::current_dir().unwrap().join("config.json"));
    assert_eq!(relative, absolute);
}

/// A process ID above every platform's maximum, so never a running process.
#[cfg(unix)]
const DEAD_PROCESS_ID: u32 = u32::MAX - 1;

#[cfg(unix)]
fn private_session_directory() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("sessions");
    crate::private_fs::create_private_dir(&directory).unwrap();
    (root, directory)
}

#[cfg(unix)]
fn plant_endpoint(path: &Path, process_id: u32, socket_path: PathBuf) {
    let endpoint = ControlEndpoint {
        version: CONTROL_VERSION,
        process_id,
        socket_path,
        token: "token".to_owned(),
    };
    crate::private_fs::write_private_file(path, &serde_json::to_vec(&endpoint).unwrap()).unwrap();
}

#[cfg(unix)]
#[test]
fn reaping_a_dead_endpoint_never_unlinks_the_socket_path_it_names() {
    let (root, directory) = private_session_directory();
    let victim = root.path().join("victim");
    fs::write(&victim, b"keep").unwrap();
    let endpoint = directory.join(format!("control-{DEAD_PROCESS_ID}.json"));
    plant_endpoint(&endpoint, DEAD_PROCESS_ID, victim.clone());

    assert!(live_control_endpoints_in(&directory).unwrap().is_empty());
    assert!(live_control_endpoint_in(&directory, DEAD_PROCESS_ID).is_err());
    assert_eq!(fs::read(&victim).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn reaping_a_dead_endpoint_removes_only_a_socket_at_its_own_socket_name() {
    let (_root, directory) = private_session_directory();
    let endpoint = directory.join(format!("control-{DEAD_PROCESS_ID}.json"));
    let socket = control_socket_path(&endpoint);
    plant_endpoint(&endpoint, DEAD_PROCESS_ID, socket.clone());
    fs::write(&socket, b"not a socket").unwrap();

    assert!(live_control_endpoints_in(&directory).unwrap().is_empty());
    assert!(!endpoint.exists(), "the dead endpoint is reaped");
    assert!(socket.exists(), "a file that is not a socket is left alone");

    fs::remove_file(&socket).unwrap();
    drop(UnixListener::bind(&socket).unwrap());
    plant_endpoint(&endpoint, DEAD_PROCESS_ID, socket.clone());
    assert!(
        live_control_endpoint_in(&directory, DEAD_PROCESS_ID)
            .unwrap()
            .is_none()
    );
    assert!(!endpoint.exists() && fs::symlink_metadata(&socket).is_err());
}

#[cfg(unix)]
#[test]
fn a_symlinked_endpoint_file_is_not_trusted() {
    let (root, directory) = private_session_directory();
    let process_id = std::process::id();
    let endpoint = directory.join(format!("control-{process_id}.json"));
    let planted = root.path().join("planted.json");
    plant_endpoint(&planted, process_id, control_socket_path(&endpoint));
    std::os::unix::fs::symlink(&planted, &endpoint).unwrap();

    assert!(live_control_endpoints_in(&directory).unwrap().is_empty());
    assert!(live_control_endpoint_in(&directory, process_id).is_err());
}

#[cfg(unix)]
#[test]
fn an_endpoint_naming_a_socket_other_than_its_own_is_not_used() {
    let (root, directory) = private_session_directory();
    let process_id = std::process::id();
    let endpoint = directory.join(format!("control-{process_id}.json"));
    plant_endpoint(&endpoint, process_id, root.path().join("attacker.sock"));

    assert!(live_control_endpoints_in(&directory).unwrap().is_empty());
    assert!(live_control_endpoint_in(&directory, process_id).is_err());

    plant_endpoint(&endpoint, process_id, control_socket_path(&endpoint));
    assert_eq!(live_control_endpoints_in(&directory).unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn endpoints_are_not_read_from_a_directory_others_can_write() {
    use std::os::unix::fs::PermissionsExt as _;

    let (_root, directory) = private_session_directory();
    let process_id = std::process::id();
    let endpoint = directory.join(format!("control-{process_id}.json"));
    plant_endpoint(&endpoint, process_id, control_socket_path(&endpoint));
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();

    assert!(live_control_endpoints_in(&directory).is_err());
    assert!(live_control_endpoint_in(&directory, process_id).is_err());
}

#[cfg(unix)]
#[test]
fn publishing_an_endpoint_does_not_write_through_a_planted_temporary_name() {
    let (root, directory) = private_session_directory();
    let victim = root.path().join("victim");
    fs::write(&victim, b"keep").unwrap();
    let endpoint = directory.join("control-1.json");
    std::os::unix::fs::symlink(&victim, endpoint.with_extension("json.tmp")).unwrap();

    write_endpoint(
        &endpoint,
        &ControlEndpoint {
            version: CONTROL_VERSION,
            process_id: 1,
            socket_path: control_socket_path(&endpoint),
            token: "token".to_owned(),
        },
    )
    .unwrap();

    assert_eq!(fs::read(&victim).unwrap(), b"keep");
    assert!(
        !fs::symlink_metadata(&endpoint)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
