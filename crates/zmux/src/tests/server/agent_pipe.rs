use super::*;
use std::io::Cursor;

/// One end of a connection: reads from a script, records what is written.
struct Duplex {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl Duplex {
    fn new(input: Vec<u8>) -> Self {
        Self {
            input: Cursor::new(input),
            output: Vec::new(),
        }
    }
}

impl Read for Duplex {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for Duplex {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(body);
    frame
}

#[test]
fn a_published_relay_pipe_is_asked_before_the_panes_own_agent() {
    let candidates = upstream_candidates(
        Some("\\\\.\\pipe\\zosh-agent-1-abc\r\n"),
        Some(Path::new(r"\\.\pipe\openssh-ssh-agent")),
    );
    assert_eq!(
        candidates,
        [
            PathBuf::from(r"\\.\pipe\zosh-agent-1-abc"),
            PathBuf::from(r"\\.\pipe\openssh-ssh-agent"),
        ]
    );
}

#[test]
fn without_a_relay_the_pane_keeps_the_agent_it_would_have_had() {
    assert_eq!(
        upstream_candidates(None, Some(Path::new(r"\\.\pipe\custom-agent"))),
        [
            PathBuf::from(r"\\.\pipe\custom-agent"),
            PathBuf::from(WINDOWS_OPENSSH_AGENT),
        ]
    );
    assert_eq!(
        upstream_candidates(None, None),
        [PathBuf::from(WINDOWS_OPENSSH_AGENT)]
    );
}

#[test]
fn a_pane_pipe_is_never_relayed_into_itself() {
    let own = crate::paths::new_pane_forwarded_agent_pipe(3).unwrap();
    let own = own.to_string_lossy();
    assert_eq!(
        upstream_candidates(Some(&own), Some(Path::new(&*own))),
        [PathBuf::from(WINDOWS_OPENSSH_AGENT)]
    );
    assert_eq!(
        upstream_candidates(Some("/tmp/agent.sock"), None),
        [PathBuf::from(WINDOWS_OPENSSH_AGENT)]
    );
}

#[test]
fn every_request_gets_the_agents_reply_in_order() {
    let mut client = Duplex::new([frame(&[11]), frame(&[13, 1, 2])].concat());
    let mut agent = Duplex::new([frame(&[12, 0, 0, 0, 0]), frame(&[14, 9])].concat());

    relay_frames(&mut client, &mut agent).unwrap();

    assert_eq!(agent.output, [frame(&[11]), frame(&[13, 1, 2])].concat());
    assert_eq!(
        client.output,
        [frame(&[12, 0, 0, 0, 0]), frame(&[14, 9])].concat()
    );
}

#[test]
fn an_oversized_request_ends_the_connection_without_reaching_the_agent() {
    let mut client = Duplex::new((MAX_FRAME as u32).to_be_bytes().to_vec());
    let mut agent = Duplex::new(Vec::new());

    let error = relay_frames(&mut client, &mut agent).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(agent.output.is_empty());
}
