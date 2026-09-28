use super::*;

#[cfg(windows)]
#[test]
fn detached_child_closes_bootstrap_stdout_before_session_ends() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    const CHILD_MARKER: &str = "ZOSH_TEST_DETACHED_STDOUT_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        println!("MOSH CONNECT 60000 AAAAAAAAAAAAAAAAAAAAAA");
        std::io::stdout().flush().unwrap();
        let _null = release_bootstrap_stdout().unwrap();
        std::thread::sleep(Duration::from_secs(10));
        return;
    }

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "lifecycle::tests::detached_child_closes_bootstrap_stdout_before_session_ends",
            "--nocapture",
        ])
        .env(CHILD_MARKER, "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        stdout.read_to_string(&mut output).unwrap();
        sender.send(output).unwrap();
    });
    let result = receiver.recv_timeout(Duration::from_secs(3));
    let still_running = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    reader.join().unwrap();

    let output = result.expect("detached child kept SSH bootstrap stdout open");
    assert!(output.contains("MOSH CONNECT 60000"), "{output}");
    assert!(still_running, "the child ended before the pipe closed");
}
