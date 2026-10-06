//! SSH provenance for processes created through a remote viewer.
//!
//! A daemon may have been started by a local window and outlive many SSH
//! logins. Its environment cannot tell a new pane that its viewer is remote.
//! Carry the actual login's SSH_CONNECTION into spawn requests so terminal
//! applications forward copies even when the remote native clipboard works.
//! Other requests do not query or change the process environment.

use super::*;

impl RemoteTransport {
    pub(crate) fn inherit_login_context(&self, request: &mut Request) -> Result<()> {
        if !matches!(
            request,
            Request::CreateShared(_) | Request::SpawnShared(_) | Request::SpawnSharedBatch(_)
        ) {
            return Ok(());
        }
        if let Some(connection) = self.login_connection()? {
            apply_connection(request, &connection);
        }
        Ok(())
    }

    fn login_connection(&self) -> Result<Option<String>> {
        let control = self.command_control()?;
        let cached = self.lock_state().login_connection.clone();
        if let Some(connection) = cached {
            return Ok(connection);
        }
        let (_, output) = self.run_host_command(control.as_deref(), |platform| {
            command_arguments(
                &self.target,
                control.as_deref(),
                connection_command(platform),
            )
        })?;
        let connection = parse_connection(&output.stdout)?;
        self.lock_state().login_connection = Some(connection.clone());
        Ok(connection)
    }
}

fn connection_command(platform: HostPlatform) -> String {
    match platform {
        HostPlatform::Posix => "/bin/sh -c 'printf \"%s\\n\" \"$SSH_CONNECTION\"'".to_owned(),
        HostPlatform::Windows => remote_host::encoded(
            "[Console]::Out.WriteLine([Environment]::GetEnvironmentVariable('SSH_CONNECTION'))",
        ),
    }
}

fn parse_connection(output: &[u8]) -> Result<Option<String>> {
    let connection = std::str::from_utf8(output)
        .context("remote SSH connection information was not UTF-8")?
        .trim();
    if connection.is_empty() {
        return Ok(None);
    }
    let fields = connection.split_whitespace().collect::<Vec<_>>();
    anyhow::ensure!(
        fields.len() == 4
            && fields[0].parse::<std::net::IpAddr>().is_ok()
            && fields[1].parse::<u16>().is_ok_and(|port| port != 0)
            && fields[2].parse::<std::net::IpAddr>().is_ok()
            && fields[3].parse::<u16>().is_ok_and(|port| port != 0),
        "remote host returned invalid SSH connection information"
    );
    Ok(Some(connection.to_owned()))
}

fn apply_connection(request: &mut Request, connection: &str) {
    let add = |env: &mut HashMap<String, String>| {
        env.insert("SSH_CONNECTION".to_owned(), connection.to_owned());
    };
    match request {
        Request::SpawnShared(request) => add(&mut request.env),
        Request::CreateShared(request) => {
            request.panes.iter_mut().for_each(|pane| add(&mut pane.env))
        }
        Request::SpawnSharedBatch(request) => {
            request.panes.iter_mut().for_each(|pane| add(&mut pane.env));
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "../tests/remote/login_context.rs"]
mod tests;
