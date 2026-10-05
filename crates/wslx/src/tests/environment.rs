use super::*;

#[test]
fn a_named_pipe_is_recognised_in_either_slash_style() {
    let pipe = r"\\.\pipe\zosh-agent-20412-18dbc5145f62dd20";
    assert_eq!(agent_pipe(OsStr::new(pipe)).as_deref(), Some(pipe));
    assert_eq!(
        agent_pipe(OsStr::new("//./pipe/openssh-ssh-agent")).as_deref(),
        Some(r"\\.\pipe\openssh-ssh-agent")
    );
    assert_eq!(
        agent_pipe(OsStr::new(" \\\\.\\PIPE\\agent \r\n")).as_deref(),
        Some(r"\\.\pipe\agent")
    );
}

#[test]
fn anything_but_a_named_pipe_is_not_carried() {
    for value in [
        "",
        r"\\.\pipe\",
        "/tmp/ssh-XXXX/agent.123",
        r"C:\Users\me\agent.sock",
        r"\\server\pipe\agent",
    ] {
        assert_eq!(agent_pipe(OsStr::new(value)), None, "{value}");
    }
}

#[test]
fn the_agent_is_appended_to_an_existing_wslenv() {
    assert_eq!(wslenv_with_agent(None), "SSH_AUTH_SOCK");
    assert_eq!(wslenv_with_agent(Some("")), "SSH_AUTH_SOCK");
    assert_eq!(
        wslenv_with_agent(Some("USERPROFILE/p:GOPATH/l")),
        "USERPROFILE/p:GOPATH/l:SSH_AUTH_SOCK"
    );
}

#[test]
fn an_existing_agent_entry_is_replaced_so_its_path_is_not_translated() {
    assert_eq!(
        wslenv_with_agent(Some("SSH_AUTH_SOCK/p:TERM")),
        "TERM:SSH_AUTH_SOCK"
    );
    assert_eq!(
        wslenv_with_agent(Some("ssh_auth_sock/u::TERM:")),
        "TERM:SSH_AUTH_SOCK"
    );
}
