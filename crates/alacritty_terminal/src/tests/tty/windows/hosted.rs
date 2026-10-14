use super::*;
use crate::event::WindowSize;

struct Ended;
impl HostedConsole for Ended {
    fn child_pid(&self) -> u32 {
        0
    }
    fn child_handle(&self) -> io::Result<std::os::windows::io::OwnedHandle> {
        Err(io::ErrorKind::Unsupported.into())
    }
    fn input(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn resize(&mut self, _: WindowSize) -> io::Result<()> {
        Ok(())
    }
    fn close(&mut self) {}
}

#[test]
fn a_reply_to_an_ended_console_is_dropped_rather_than_failing_the_backend() {
    let console: Box<dyn HostedConsole> = Box::new(Ended);
    let mut writer = PtyWriter::Hosted(Arc::new(Mutex::new(console)));
    assert_eq!(writer.write(b"\x1b[1;1R").unwrap(), 6);
}
