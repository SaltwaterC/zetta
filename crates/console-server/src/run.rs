//! Ordered probe lifecycle: own a console, launch a child, serve, then report.
//! A watchdog bounds hangs and the job only contains this run's descendants.
use crate::native::{Child, Console};
use crate::options::Options;
use crate::server::Server;
use crate::sink::Sink;
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use windows_sys::Win32::System::JobObjects::TerminateJobObject;

pub fn run(options: Options) -> io::Result<()> {
    if options.control_child {
        return crate::control::child_probe();
    }
    let sink = Sink::new(options.drain, options.capture.as_deref(), options.history)?;
    let mut server = Server::new(sink, options.ctrl_c);
    let console = Console::new()?;
    let command = match options.command {
        Some(command) => command,
        None => format!("\"{}\" --control-child", std::env::current_exe()?.display()),
    };
    let started = Instant::now();
    let child = Child::spawn(&console, &command)?;
    let (done, wait) = mpsc::channel::<()>();
    let timed_out = Arc::new(AtomicBool::new(false));
    let timeout_flag = Arc::clone(&timed_out);
    let job = Arc::clone(&child.job);
    let timeout = Duration::from_secs(options.timeout);
    let watchdog = std::thread::spawn(move || {
        if matches!(
            wait.recv_timeout(timeout),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            timeout_flag.store(true, Ordering::Release);
            unsafe {
                TerminateJobObject(job.raw(), 124);
            }
        }
    });
    // The child now owns the reference; EOF arrives only when its whole console
    // client tree has disconnected, so output isn't truncated at root-process exit.
    drop(console.reference);
    let served = server.serve(&console.server);
    let _ = done.send(());
    watchdog
        .join()
        .map_err(|_| io::Error::other("watchdog panicked"))?;
    if timed_out.load(Ordering::Acquire) {
        return Err(io::Error::other(format!(
            "child console timed out after {} seconds",
            options.timeout
        )));
    }
    served?;
    let exit_code = child.exit_code()?;
    let elapsed = started.elapsed().as_secs_f64();
    let control_status = server
        .control
        .take()
        .map(|thread| {
            thread
                .join()
                .map_err(|_| io::Error::other("control probe panicked"))?
        })
        .transpose()?;
    let report = format!(
        "{{\n  \"version\": 1,\n  \"backend\": \"console-server\",\n  \"parser\": {},\n  \"history\": {},\n  \"elapsed_seconds\": {:.6},\n  \"source_bytes\": {},\n  \"parser_bytes\": {},\n  \"mib_per_second\": {:.3},\n  \"requests\": {},\n  \"writes\": {},\n  \"connected_processes\": {},\n  \"child_exit_code\": {},\n  \"unsupported_apis\": {:?},\n  \"ctrl_c_status\": {},\n  \"ctrl_c_handled\": {}\n}}\n",
        !options.drain,
        options.history,
        elapsed,
        server.source_bytes,
        server.sink.bytes(),
        server.source_bytes as f64 / 1048576.0 / elapsed,
        server.requests,
        server.writes,
        server.connected,
        exit_code,
        server.unsupported.iter().collect::<Vec<_>>(),
        control_status.map_or("null".to_owned(), |status| status.to_string()),
        server.ctrl_handled
    );
    if let Some(path) = options.report {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &report)?;
    }
    print!("{report}");
    if exit_code != 0 {
        return Err(io::Error::other(format!(
            "child {} exited with {exit_code}",
            child.pid
        )));
    }
    if options.ctrl_c && !server.ctrl_handled {
        return Err(io::Error::other(
            "Ctrl+C did not reach the child's registered handler",
        ));
    }
    Ok(())
}
