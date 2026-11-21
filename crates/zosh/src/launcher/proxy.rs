//! `zosh --proxy`: relaying the bootstrap's stdio to a TCP connection, as
//! an SSH `ProxyCommand`.

use super::*;

pub(super) fn run_proxy(command: &MoshCommand, proxy: &ProxyRequest) -> Result<()> {
    let addresses = (proxy.host.as_str(), proxy.port)
        .to_socket_addrs()
        .with_context(|| format!("resolving proxy target {:?}", proxy.host))?
        .collect::<Vec<_>>();
    let candidates = ordered_socket_addresses(&addresses, command.family)
        .context("proxy target did not resolve to the requested address family")?;
    let (address, stream) = connect_proxy(candidates)?;
    eprintln!("MOSH IP {}", address.ip());
    #[cfg(unix)]
    {
        run_proxy_unix(stream)
    }
    #[cfg(not(unix))]
    {
        run_proxy_threaded(stream)
    }
}

pub(super) fn copy_proxy_output<R: Read>(reader: &mut R) -> io::Result<u64> {
    let mut stdout = io::stdout();
    copy_proxy_output_to(reader, &mut stdout)
}

pub(super) fn copy_proxy_output_to<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 16 * 1024];
    let mut copied = 0;
    loop {
        let length = reader.read(&mut buffer)?;
        if length == 0 {
            return Ok(copied);
        }
        writer.write_all(&buffer[..length])?;
        writer.flush()?;
        copied += length as u64;
    }
}

#[cfg(unix)]
pub(super) fn run_proxy_unix(mut stream: TcpStream) -> Result<()> {
    // The stock Mosh proxy forks so the output side can close its stdout as
    // soon as the TCP peer closes. That EOF is how OpenSSH learns that its
    // ProxyCommand has ended; keeping both directions in threads leaves the
    // parent waiting on stdin forever after a remote disconnect.
    //
    // No threads have been created by the proxy path before this point, so
    // the child can safely perform the small amount of I/O below before it
    // exits with `_exit`.
    // SAFETY: this path has not created any threads, and the child only uses
    // inherited file descriptors before terminating with `_exit`.
    let child = unsafe { libc::fork() };
    if child == -1 {
        return Err(io::Error::last_os_error()).context("forking the Mosh proxy");
    }
    if child == 0 {
        // The output child must not keep the proxy input pipe open: doing so
        // prevents OpenSSH from observing EOF when the remote socket closes.
        // SAFETY: fd 0 is the proxy command's stdin and is valid or already
        // closed; either result is harmless in this terminating child.
        unsafe { libc::close(libc::STDIN_FILENO) };
        let result = copy_proxy_output(&mut stream);
        let _ = stream.shutdown(Shutdown::Read);
        // SAFETY: this is the fork child and must not run the parent's Rust
        // destructors or flush unrelated process state.
        unsafe { libc::_exit(i32::from(result.is_err())) };
    }

    // Match the stock proxy: a controlling-terminal hangup must not kill the
    // input half while OpenSSH is still using this process as its transport.
    // SAFETY: changing this process-local signal disposition is safe before
    // the proxy returns; the proxy is a short-lived helper process.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::close(libc::STDOUT_FILENO);
    }
    let mut stdin = io::stdin();
    let input_result = io::copy(&mut stdin, &mut stream);
    let _ = stream.shutdown(Shutdown::Write);

    let mut status = 0;
    let wait_result = loop {
        // SAFETY: `child` is the PID returned by `fork`, `status` is a valid
        // out-parameter, and waiting for this direct child is intentional.
        let result = unsafe { libc::waitpid(child, &mut status, 0) };
        if result == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        break result;
    };
    input_result.context("forwarding Mosh proxy input")?;
    anyhow::ensure!(
        wait_result == child,
        "waiting for the Mosh proxy output child"
    );
    anyhow::ensure!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "Mosh proxy output failed"
    );
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn run_proxy_threaded(mut stream: TcpStream) -> Result<()> {
    let mut remote = stream
        .try_clone()
        .context("cloning the Mosh proxy stream")?;
    let reader = thread::spawn(move || {
        let result = copy_proxy_output(&mut remote);
        if let Err(error) = &result {
            eprintln!("zosh: Mosh proxy output failed: {error}");
        }
        // On platforms without fork, terminating the helper when the output
        // half closes is the equivalent of the stock proxy child exiting and
        // releasing OpenSSH's stdout pipe.
        std::process::exit(i32::from(result.is_err()));
    });
    let mut stdin = io::stdin();
    let input_result = io::copy(&mut stdin, &mut stream);
    let _ = stream.shutdown(Shutdown::Write);
    let _ = reader.join();
    input_result.context("forwarding Mosh proxy input")?;
    Ok(())
}

pub(super) fn connect_proxy(candidates: Vec<SocketAddr>) -> Result<(SocketAddr, TcpStream)> {
    let mut last_error = None;
    for address in candidates {
        match TcpStream::connect(address) {
            Ok(stream) => return Ok((address, stream)),
            Err(error) => last_error = Some(error),
        }
    }
    Err(anyhow::anyhow!(
        "could not connect the Mosh proxy{}",
        last_error
            .map(|error| format!(": {error}"))
            .unwrap_or_default()
    ))
}
